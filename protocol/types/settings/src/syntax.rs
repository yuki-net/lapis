//! 既存設定の値型。保存ファイルと定義データの両方から参照する。

use serde::{Deserialize, Serialize};

/// 構文配色の系統。明暗はUIテーマに追従する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyntaxTheme {
    #[serde(rename = "vscode", alias = "vscode.dark-plus")]
    VsCode,
    #[serde(alias = "zed.one-dark")]
    Zed,
    #[serde(alias = "intellij.dark")]
    Intellij,
    #[serde(alias = "intellij.darcula")]
    IntellijClassic,
    #[default]
    #[serde(alias = "lapis.dark", alias = "lapis.light", other)]
    Lapis,
}

impl SyntaxTheme {
    /// 保存・定義データで使う正規ID。
    pub const fn as_id(self) -> &'static str {
        match self {
            Self::Lapis => "lapis",
            Self::VsCode => "vscode",
            Self::Zed => "zed",
            Self::Intellij => "intellij",
            Self::IntellijClassic => "intellij-classic",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [
            Self::Lapis,
            Self::VsCode,
            Self::Zed,
            Self::Intellij,
            Self::IntellijClassic,
        ]
        .into_iter()
        .find(|theme| theme.as_id() == id)
    }
}

/// ターミナルのカーソル形状。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalCaretShape {
    #[default]
    Block,
    Bar,
    Underline,
}

impl TerminalCaretShape {
    pub const fn as_id(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Bar => "bar",
            Self::Underline => "underline",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Block, Self::Bar, Self::Underline]
            .into_iter()
            .find(|shape| shape.as_id() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_palette_ids_migrate_to_families() {
        for (old, family, canonical) in [
            ("lapis.dark", SyntaxTheme::Lapis, "lapis"),
            ("lapis.light", SyntaxTheme::Lapis, "lapis"),
            ("vscode.dark-plus", SyntaxTheme::VsCode, "vscode"),
            ("zed.one-dark", SyntaxTheme::Zed, "zed"),
            ("intellij.dark", SyntaxTheme::Intellij, "intellij"),
            (
                "intellij.darcula",
                SyntaxTheme::IntellijClassic,
                "intellij-classic",
            ),
        ] {
            let json = format!("\"{old}\"");
            let theme: SyntaxTheme = serde_json::from_str(&json).unwrap();
            assert_eq!(theme, family);
            assert_eq!(serde_json::to_value(theme).unwrap(), canonical);
        }
        let unknown: SyntaxTheme = serde_json::from_str(r#""missing""#).unwrap();
        assert_eq!(unknown, SyntaxTheme::Lapis);
    }

    #[test]
    fn ids_round_trip() {
        for theme in [
            SyntaxTheme::Lapis,
            SyntaxTheme::VsCode,
            SyntaxTheme::Zed,
            SyntaxTheme::Intellij,
            SyntaxTheme::IntellijClassic,
        ] {
            assert_eq!(SyntaxTheme::from_id(theme.as_id()), Some(theme));
        }
        for shape in [
            TerminalCaretShape::Block,
            TerminalCaretShape::Bar,
            TerminalCaretShape::Underline,
        ] {
            assert_eq!(TerminalCaretShape::from_id(shape.as_id()), Some(shape));
        }
        assert_eq!(TerminalCaretShape::from_id("missing"), None);
    }
}
