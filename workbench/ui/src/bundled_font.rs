//! Web 版に同梱する等幅フォント。
//!
//! Web の GPUI は空の fontdb から始まり、OS やブラウザーのフォントを引けない。
//! 使えるのはここで読み込んだフォントだけなので、`monospace` のような総称名は
//! 解決できず、読み込んだ UI フォント (IBM Plex Sans) へ落ちてしまう。その状態では
//! 等幅にならず、罫線・ブロック要素のグリフも無いため豆腐 (□) になる。
//!
//! そこで Desktop (Windows) の既定と同じ Cascadia Mono を同梱し、Web の既定
//! family にする。罫線 (U+2500–U+257F)・ブロック要素 (U+2580–U+259F)・点字
//! (U+2800–U+28FF) を 1 セル幅で持つフォントを選んでいる。

/// 同梱フォントの family 名。GPUI はこの名前でフォントを引く。
pub(crate) const FAMILY: &str = "Cascadia Mono";

/// 起動時に読み込む同梱フォント。通常表示と強調表示を賄う。
#[cfg(target_family = "wasm")]
pub(crate) fn fonts() -> Vec<std::borrow::Cow<'static, [u8]>> {
    vec![
        std::borrow::Cow::Borrowed(include_bytes!("../assets/CascadiaMono-Regular.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/CascadiaMono-Bold.ttf")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同梱ファイルを直接読む。非 wasm のビルドへフォントを埋め込まないため、
    /// 検証も `include_bytes!` ではなく実ファイルを対象にする。
    const FILES: [&str; 2] = ["CascadiaMono-Regular.ttf", "CascadiaMono-Bold.ttf"];

    fn asset(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn face(data: &[u8]) -> ttf_parser::Face<'_> {
        ttf_parser::Face::parse(data, 0).expect("同梱フォントを解析できる")
    }

    fn family_of(data: &[u8]) -> String {
        face(data)
            .names()
            .into_iter()
            .find(|name| name.name_id == ttf_parser::name_id::FAMILY && name.is_unicode())
            .and_then(|name| name.to_string())
            .expect("family 名を持つ")
    }

    /// 同梱フォントが GPUI の family 名で引けることを保証する。
    /// 名前がずれると Web の Terminal は既定の別フォントへ落ちて等幅でなくなる。
    #[test]
    fn bundled_fonts_use_the_resolved_family() {
        for name in FILES {
            assert_eq!(family_of(&asset(name)), FAMILY, "{name}");
        }
    }

    /// ターミナルの罫線・ブロック要素・点字が欠けないことを保証する。
    /// これらが無いと Web では豆腐 (□) になる。
    #[test]
    fn bundled_fonts_cover_terminal_glyphs() {
        for name in FILES {
            let data = asset(name);
            let face = face(&data);
            for code in (0x2500..=0x259f).chain(0x2800..=0x28ff) {
                let character = char::from_u32(code).expect("有効な文字コード");
                assert!(
                    face.glyph_index(character).is_some(),
                    "{name} に U+{code:04X} のグリフが無い"
                );
            }
        }
    }

    /// セル幅を 1 文字の送り幅から決めるため、ASCII が等幅であることを保証する。
    /// 罫線とブロック要素も同じ送り幅でないとセル境界からずれる。
    #[test]
    fn bundled_fonts_are_monospace() {
        for name in FILES {
            let data = asset(name);
            let face = face(&data);
            let advance = |character: char| {
                face.glyph_index(character)
                    .and_then(|glyph| face.glyph_hor_advance(glyph))
                    .expect("対象グリフの送り幅がある")
            };
            let expected = advance('M');
            for character in ['i', 'W', '0', ' ', '\u{2500}', '\u{2588}'] {
                assert_eq!(advance(character), expected, "{name} の {character:?}");
            }
        }
    }
}
