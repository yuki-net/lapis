use std::collections::BTreeMap;

use lapis_document::Document;

/// 色・フォントに依存しない既存の保守的な幅推定。描画時は最大値だけを読む。
#[derive(Default)]
pub(super) struct LineWidths {
    document: Document,
    lines: Vec<usize>,
    counts: BTreeMap<usize, usize>,
}

impl LineWidths {
    pub(super) fn new(document: &Document) -> Self {
        let mut widths = Self::default();
        widths.replace(0..0, document, 0..document.len_lines());
        widths
    }

    pub(super) fn max(&self) -> f32 {
        self.counts.last_key_value().map_or(0, |(w, _)| *w) as f32 * 18.0
    }

    pub(super) fn refresh(&mut self, document: &Document) {
        let mut first = 0;
        while first < self.lines.len().min(document.len_lines())
            && self.document.line(first) == document.line(first)
        {
            first += 1;
        }
        let mut suffix = 0;
        while suffix < (self.lines.len() - first).min(document.len_lines() - first)
            && self.document.line(self.lines.len() - suffix - 1)
                == document.line(document.len_lines() - suffix - 1)
        {
            suffix += 1;
        }
        self.replace(
            first..self.lines.len() - suffix,
            document,
            first..document.len_lines() - suffix,
        );
    }

    pub(super) fn replace(
        &mut self,
        old: std::ops::Range<usize>,
        document: &Document,
        new: std::ops::Range<usize>,
    ) {
        let replacement = new
            .map(|line| {
                document
                    .line(line)
                    .unwrap_or_default()
                    .trim_end_matches(['\r', '\n'])
                    .chars()
                    .count()
            })
            .collect::<Vec<_>>();
        for width in &self.lines[old.clone()] {
            let count = self.counts.get_mut(width).expect("cached width");
            *count -= 1;
            if *count == 0 {
                self.counts.remove(width);
            }
        }
        for width in &replacement {
            *self.counts.entry(*width).or_default() += 1;
        }
        self.lines.splice(old, replacement);
        self.document = document.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_deleting_and_unicode_widths() {
        let mut doc = Document::new();
        doc.replace_char_range(0..0, "longest line\r\n日😀\nxx")
            .unwrap();
        let mut widths = LineWidths::new(&doc);
        assert_eq!(widths.max(), 216.0);
        doc.replace_char_range(0..12, "a").unwrap();
        widths.replace(0..1, &doc, 0..1);
        assert_eq!(widths.max(), 36.0);
        let end = doc.len_chars();
        doc.replace_char_range(3..end, "").unwrap();
        widths.replace(1..3, &doc, 1..doc.len_lines());
        assert_eq!(widths.max(), 18.0);
        assert!(doc.undo());
        widths.refresh(&doc);
        assert_eq!(widths.max(), 36.0);
        assert!(doc.redo());
        widths.refresh(&doc);
        assert_eq!(widths.max(), 18.0);
        widths.refresh(&Document::new());
        assert_eq!(widths.max(), 0.0);
    }

    #[test]
    #[ignore = "手動性能測定: cargo test -p lapis-desktop-ui measure_editor -- --ignored --nocapture"]
    fn measure_editor_widths() {
        use std::{hint::black_box, time::Instant};
        for count in [1_000, 10_000, 100_000] {
            let mut doc = Document::new();
            doc.replace_char_range(0..0, &"fn main() { let s = \"日本😀\"; }\r\n".repeat(count))
                .unwrap();
            let start = Instant::now();
            let mut widths = LineWidths::new(&doc);
            let init = start.elapsed();
            let start = Instant::now();
            for _ in 0..100 {
                black_box(
                    (0..doc.len_lines())
                        .filter_map(|i| doc.line(i))
                        .map(|s| s.chars().count())
                        .max(),
                );
                black_box(
                    (0..doc.len_lines())
                        .filter_map(|i| doc.line(i))
                        .map(|s| {
                            s.trim_end_matches(['\r', '\n'])
                                .chars()
                                .map(|c| if c.is_ascii() { 9.0 } else { 18.0 })
                                .sum::<f32>()
                        })
                        .fold(0.0f32, f32::max),
                );
            }
            let old = start.elapsed();
            let start = Instant::now();
            for _ in 0..100 {
                black_box(widths.max());
                black_box(widths.max());
            }
            let cached = start.elapsed();
            doc.replace_char_range(0..2, "f").unwrap();
            let start = Instant::now();
            widths.replace(0..1, &doc, 0..1);
            println!(
                "lines={count} width_init={init:?} old_100_frames={old:?} cached_100_frames={cached:?} edit_width={:?}",
                start.elapsed()
            );
        }
    }
}
