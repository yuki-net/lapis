use std::{
    ops::Range,
    path::Path,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

use lapis_document::Document;
use syntect::{
    easy::ScopeRangeIterator,
    parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet},
};

// 純Rustの正規表現実装。文法の展開・初期化は解析worker上で一度だけ行う。
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINES: usize = 50_000;
const MAX_LINE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub(super) range: Range<usize>,
    pub(super) scopes: Vec<Scope>,
}

pub(super) type Lines = Vec<Arc<Vec<Span>>>;

/// 解析待ちの表示色を編集後のUTF-8範囲へ移す。解析状態そのものは変更しない。
/// 挿入文字は隣接スコープを仮に引き継ぎ、変更外の行は同じArcを保持する。
pub(super) fn rebase(
    lines: &mut Lines,
    before: &Document,
    after: &Document,
    edited: Range<usize>,
) -> Option<()> {
    if lines.len() != before.len_lines() {
        return None;
    }
    // CR / LFの結合・分離が前後の行境界にも影響する。
    let first = (before.char_to_position(edited.start).ok()?.line as usize).saturating_sub(1);
    let last =
        (before.char_to_position(edited.end).ok()?.line as usize + 1).min(before.len_lines() - 1);
    let start_char = before
        .position_to_char(lapis_document::Position::new(first as u32, 0))
        .ok()?;
    let base = before.char_to_byte(start_char).ok()?;
    let cut = before.char_to_byte(edited.start).ok()? - base
        ..before.char_to_byte(edited.end).ok()? - base;
    let inserted = after
        .len_bytes()
        .checked_sub(before.len_bytes() - cut.len())?;
    let mut old = Vec::new();
    let mut offset = 0;
    for (index, spans) in lines.iter().enumerate().take(last + 1).skip(first) {
        for span in spans.iter() {
            old.push(Span {
                range: offset + span.range.start..offset + span.range.end,
                scopes: span.scopes.clone(),
            });
        }
        offset += before.line(index)?.len();
    }
    let inherited = old
        .iter()
        .find(|s| s.range.start < cut.start && cut.start <= s.range.end)
        .or_else(|| old.first())
        .map(|s| s.scopes.clone())
        .unwrap_or_default();
    let mut rebased = Vec::new();
    for span in &old {
        push_span(
            &mut rebased,
            span.range.start..span.range.end.min(cut.start),
            &span.scopes,
        );
    }
    push_span(&mut rebased, cut.start..cut.start + inserted, &inherited);
    for span in &old {
        let start = span.range.start.max(cut.end);
        if start < span.range.end {
            push_span(
                &mut rebased,
                start - cut.end + cut.start + inserted
                    ..span.range.end - cut.end + cut.start + inserted,
                &span.scopes,
            );
        }
    }
    let new_end = (last + 1)
        .checked_add(after.len_lines())?
        .checked_sub(before.len_lines())?;
    let mut replacement = Vec::new();
    let mut byte = 0;
    let mut span_index = 0;
    for index in first..new_end {
        let raw = after.line(index)?;
        let end = byte + raw.len();
        let mut spans = Vec::new();
        while span_index < rebased.len() && rebased[span_index].range.start < end {
            let span = &rebased[span_index];
            let start = span.range.start.max(byte) - byte;
            let stop = span.range.end.min(end) - byte;
            if !raw.is_char_boundary(start) || !raw.is_char_boundary(stop) {
                return None;
            }
            push_span(&mut spans, start..stop, &span.scopes);
            if span.range.end > end {
                break;
            }
            span_index += 1;
        }
        if spans.iter().map(|s| s.range.len()).sum::<usize>() != raw.len() {
            return None;
        }
        replacement.push(Arc::new(spans));
        byte = end;
    }
    lines.splice(first..last + 1, replacement);
    Some(())
}

fn push_span(spans: &mut Vec<Span>, range: Range<usize>, scopes: &[Scope]) {
    if range.start >= range.end {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.range.end == range.start
        && last.scopes == scopes
    {
        last.range.end = range.end;
    } else {
        spans.push(Span {
            range,
            scopes: scopes.to_vec(),
        });
    }
}

struct ParsedLine {
    text: String,
    parser: ParseState,
    stack: ScopeStack,
    spans: Arc<Vec<Span>>,
}

#[derive(Default)]
pub(super) struct SyntaxCache {
    syntax: Option<String>,
    lines: Vec<ParsedLine>,
    pub(super) parsed: usize,
}

fn detect(path: Option<&Path>, first: &str) -> &'static SyntaxReference {
    path.and_then(|p| p.file_name())
        .and_then(|name| name.to_str())
        .and_then(|name| {
            SYNTAXES.find_syntax_by_extension(name).or_else(|| {
                Path::new(name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(|ext| SYNTAXES.find_syntax_by_extension(ext))
            })
        })
        .or_else(|| SYNTAXES.find_syntax_by_first_line(first))
        .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text())
}

impl SyntaxCache {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// 変更行から再開し、同じ接尾部とparser/scope状態に収束したら残りを再利用する。
    /// 中断時は完成済みの行だけ残し、解析失敗時は単色へ戻す。
    pub(super) fn update(
        &mut self,
        document: &Document,
        generation: u64,
        latest: &AtomicU64,
    ) -> Option<Lines> {
        self.parsed = 0;
        if document.len_bytes() > MAX_BYTES || document.len_lines() > MAX_LINES {
            self.clear();
            return None;
        }
        let result = self.parse(document, generation, latest);
        if result.is_none() && latest.load(Ordering::Relaxed) == generation {
            self.clear();
        }
        result
    }

    fn parse(&mut self, document: &Document, generation: u64, latest: &AtomicU64) -> Option<Lines> {
        let syntax = detect(document.path(), &document.line(0).unwrap_or_default());
        if self.syntax.as_deref() != Some(&syntax.name) {
            self.clear();
            self.syntax = Some(syntax.name.clone());
        }
        let len = document.len_lines();
        let mut prefix = 0;
        while prefix < len.min(self.lines.len()) {
            if latest.load(Ordering::Relaxed) != generation {
                return None;
            }
            if document.line(prefix).as_deref() != Some(&self.lines[prefix].text) {
                break;
            }
            prefix += 1;
        }
        let mut suffix = 0;
        while suffix < (len - prefix).min(self.lines.len() - prefix) {
            if latest.load(Ordering::Relaxed) != generation {
                return None;
            }
            if document.line(len - suffix - 1).as_deref()
                != Some(&self.lines[self.lines.len() - suffix - 1].text)
            {
                break;
            }
            suffix += 1;
        }
        let mut parser = if prefix == 0 {
            ParseState::new(syntax)
        } else {
            self.lines[prefix - 1].parser.clone()
        };
        let mut stack = if prefix == 0 {
            ScopeStack::new()
        } else {
            self.lines[prefix - 1].stack.clone()
        };
        let old_len = self.lines.len();
        let mut tail = self.lines.split_off(prefix);
        for index in prefix..len {
            if latest.load(Ordering::Relaxed) != generation {
                return None;
            }
            let text = document.line(index)?;
            // 長行を飛ばしてparser状態を推測しない。文書全体を単色へ戻す。
            if text.len() > MAX_LINE_BYTES {
                return None;
            }
            let ops = parser.parse_line(&text, &SYNTAXES).ok()?;
            self.parsed += 1;
            let mut spans = Vec::new();
            for (range, op) in ScopeRangeIterator::new(&ops, &text) {
                stack.apply(op).ok()?;
                if !range.is_empty() {
                    if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
                        return None;
                    }
                    spans.push(Span {
                        range,
                        scopes: stack.as_slice().to_vec(),
                    });
                }
            }
            self.lines.push(ParsedLine {
                text,
                parser: parser.clone(),
                stack: stack.clone(),
                spans: Arc::new(spans),
            });
            if index >= len - suffix {
                let old_index = old_len - (len - index) - prefix;
                if parser == tail[old_index].parser && stack == tail[old_index].stack {
                    self.lines.extend(tail.drain(old_index + 1..));
                    break;
                }
            }
        }
        Some(self.lines.iter().map(|line| line.spans.clone()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_document::{Encoding, Revision};

    fn doc(path: &str, text: &str) -> Document {
        Document::from_snapshot(
            Some(path.into()),
            text.into(),
            Encoding::Utf8,
            Revision::new(1),
            false,
        )
    }

    fn assert_coverage(lines: &Lines, document: &Document) {
        assert_eq!(lines.len(), document.len_lines());
        for (i, spans) in lines.iter().enumerate() {
            let raw = document.line(i).unwrap();
            let mut end = 0;
            for span in spans.iter() {
                assert_eq!(span.range.start, end);
                assert!(raw.get(span.range.clone()).is_some());
                end = span.range.end;
            }
            assert_eq!(end, raw.len());
        }
    }

    #[test]
    fn typing_preserves_colors_and_unaffected_line_storage_until_parse_finishes() {
        let latest = AtomicU64::new(1);
        let text = "// header\nlet s = \"日本\";\nfn a() {}\nfn b() {}\nfn c() {}\n";
        let mut document = doc("a.rs", text);
        let mut cache = SyntaxCache::default();
        let mut lines = cache.update(&document, 1, &latest).unwrap();
        let tail = lines[4].clone();
        let cursor = document
            .byte_to_char(text.find("日本").unwrap() + "日".len())
            .unwrap();
        let original_spans = lines[1].len();
        let original_scopes = lines[1]
            .iter()
            .find(|s| {
                s.range
                    .contains(&document.line(1).unwrap().find("日本").unwrap())
            })
            .unwrap()
            .scopes
            .clone();
        for n in 0..100 {
            let before = document.clone();
            document
                .replace_char_range(cursor + n..cursor + n, "😀")
                .unwrap();
            rebase(&mut lines, &before, &document, cursor + n..cursor + n).unwrap();
            assert_coverage(&lines, &document);
            assert!(lines[1].len() <= original_spans + 2);
            assert!(Arc::ptr_eq(&tail, &lines[4]));
            let raw = document.line(1).unwrap();
            let emoji = raw.find('😀').unwrap();
            assert_eq!(
                lines[1]
                    .iter()
                    .find(|s| s.range.contains(&emoji))
                    .unwrap()
                    .scopes,
                original_scopes
            );
        }
        assert_eq!(cache.parsed, document.len_lines()); // 表示範囲の補正ではparserを呼ばない。
        assert_eq!(
            cache.update(&document, 1, &latest),
            SyntaxCache::default().update(&document, 1, &latest)
        );
    }

    #[test]
    fn provisional_ranges_cover_multiline_replacements_and_crlf_boundary_changes() {
        let latest = AtomicU64::new(1);
        let mut document = doc("a.rs", "fn main() {\r\n let x = \"日😀\";\r\n}\n");
        let mut lines = SyntaxCache::default()
            .update(&document, 1, &latest)
            .unwrap();
        for (range, replacement) in [
            (15..18, "日本\r\n😀\n"),
            (2..22, "x"),
            (0..0, "\r"),
            (1..1, "\n"),
            (0..1, ""),
            (0..0, "\r"),
        ] {
            let before = document.clone();
            document
                .replace_char_range(range.clone(), replacement)
                .unwrap();
            rebase(&mut lines, &before, &document, range).unwrap();
            assert_coverage(&lines, &document);
        }
        let before = document.clone();
        document
            .replace_char_range(0..document.len_chars(), "")
            .unwrap();
        rebase(&mut lines, &before, &document, 0..before.len_chars()).unwrap();
        assert_coverage(&lines, &document);
    }

    #[test]
    fn provisional_ranges_stay_valid_during_successive_unicode_edits() {
        let latest = AtomicU64::new(1);
        let mut document = doc("a.rs", "let s = \"日😀\";\r\n/* comment */\n");
        let mut lines = SyntaxCache::default()
            .update(&document, 1, &latest)
            .unwrap();
        let replacements = ["x", "😀", "\r", "\n", "", "日本", "/*", "\""];
        let mut seed = 7u64;
        for _ in 0..500 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let start = seed as usize % (document.len_chars() + 1);
            let end = (start + (seed >> 32) as usize % 4).min(document.len_chars());
            let replacement = replacements[(seed >> 16) as usize % replacements.len()];
            let before = document.clone();
            document
                .replace_char_range(start..end, replacement)
                .unwrap();
            rebase(&mut lines, &before, &document, start..end).unwrap();
            assert_coverage(&lines, &document);
        }
    }

    #[test]
    fn bundled_detection_and_scope_byte_boundaries() {
        let latest = AtomicU64::new(1);
        for (path, name) in [
            ("a.rs", "Rust"),
            ("a.html", "HTML"),
            ("a.css", "CSS"),
            ("a.js", "JavaScript"),
            ("a.json", "JSON"),
            ("a.md", "Markdown"),
            ("Makefile", "Makefile"),
        ] {
            assert_eq!(detect(Some(Path::new(path)), "").name, name);
        }
        // バンドルにない言語の対応をLanguageRegistryから推測しない。
        assert_eq!(
            detect(Some(Path::new("a.unknown-lapis")), "").name,
            "Plain Text"
        );
        for syntax in SYNTAXES.syntaxes() {
            for extension in &syntax.file_extensions {
                assert!(SYNTAXES.find_syntax_by_extension(extension).is_some());
            }
        }
        let document = doc(
            "a.rs",
            "/* 日本語😀\r\n comment */\nfn main() { let s = \"😀\"; }\n",
        );
        let mut cache = SyntaxCache::default();
        let lines = cache.update(&document, 1, &latest).unwrap();
        assert!(lines[1].iter().any(|s| {
            s.scopes
                .iter()
                .any(|s| s.to_string().starts_with("comment"))
        }));
        for (i, spans) in lines.iter().enumerate() {
            let raw = document.line(i).unwrap();
            assert_eq!(
                spans.iter().map(|s| s.range.len()).sum::<usize>(),
                raw.len()
            );
            for span in spans.iter() {
                assert!(raw.get(span.range.clone()).is_some());
            }
        }
        println!(
            "bundled syntaxes: {}, TypeScript: {}",
            SYNTAXES.syntaxes().len(),
            detect(Some(Path::new("a.ts")), "").name
        );
    }

    #[test]
    fn incremental_matches_fresh_after_multiline_edits_and_undo() {
        let latest = AtomicU64::new(1);
        let mut document = doc("a.rs", &"fn main() {}\n".repeat(200));
        let mut cache = SyntaxCache::default();
        cache.update(&document, 1, &latest).unwrap();
        document
            .replace_char_range(0..0, "/*\n日本😀\n*/\n")
            .unwrap();
        let incremental = cache.update(&document, 1, &latest).unwrap();
        assert!(cache.parsed < 10);
        assert_eq!(
            incremental,
            SyntaxCache::default()
                .update(&document, 1, &latest)
                .unwrap()
        );
        document.replace_char_range(0..2, "").unwrap();
        assert_eq!(
            cache.update(&document, 1, &latest),
            SyntaxCache::default().update(&document, 1, &latest)
        );
        latest.store(2, Ordering::Relaxed);
        assert!(cache.update(&document, 1, &latest).is_none());
        assert!(cache.update(&document, 2, &latest).is_some());
    }

    #[test]
    fn embedded_html_and_fallback() {
        let latest = AtomicU64::new(1);
        let document = doc(
            "a.html",
            "<style>body { color: red; }</style>\n<script>const s = '日本😀';</script>\n",
        );
        let mut cache = SyntaxCache::default();
        let result = cache.update(&document, 1, &latest).unwrap();
        assert!(result[0].iter().any(|s| {
            s.scopes
                .iter()
                .any(|s| s.to_string().contains("source.css"))
        }));
        assert!(
            result[1]
                .iter()
                .any(|s| s.scopes.iter().any(|s| s.to_string().contains("source.js")))
        );
        assert!(
            cache
                .update(&doc("a.rs", &"x".repeat(MAX_LINE_BYTES + 1)), 1, &latest)
                .is_none()
        );
    }

    #[test]
    #[ignore = "手動性能測定"]
    fn measure_editor_syntax() {
        use std::time::Instant;
        let start = Instant::now();
        LazyLock::force(&SYNTAXES);
        println!(
            "syntax_set_init={:?}, syntaxes={}",
            start.elapsed(),
            SYNTAXES.syntaxes().len()
        );
        let latest = AtomicU64::new(1);
        for count in [1_000, 10_000, 40_000] {
            let mut document = doc(
                "a.rs",
                &"fn main() { let s = \"日本😀\"; }\r\n".repeat(count),
            );
            let mut cache = SyntaxCache::default();
            let start = Instant::now();
            cache.update(&document, 1, &latest).unwrap();
            let initial = start.elapsed();
            document.replace_char_range(3..7, "test").unwrap();
            let start = Instant::now();
            cache.update(&document, 1, &latest).unwrap();
            println!(
                "lines={count} initial_parse={initial:?} edit_parse={:?} parsed_lines={}",
                start.elapsed(),
                cache.parsed
            );
        }
    }
}
