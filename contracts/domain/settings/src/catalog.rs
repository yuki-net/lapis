//! アプリ設定の定義データ。画面とログイン設定の唯一の入力。

use std::sync::OnceLock;

use crate::definition::{
    DefinitionCatalog, OptionSource, SettingCategory, SettingControl, SettingDefinition,
    SettingGroup, SettingPage, TERMINAL_CARET_SHAPE, TERMINAL_DEFAULT_PROFILE,
    TERMINAL_FONT_FAMILY, TERMINAL_FONT_SIZE, TERMINAL_LINE_HEIGHT, UI_LOCALE, UI_SYNTAX_THEME,
    UI_THEME,
};
use crate::scope::ScopeKind;
use crate::syntax::{SyntaxTheme, TerminalCaretShape};
use crate::value::{Label, SettingOption, SettingValue, ValueKind};

const USER: &[ScopeKind] = &[ScopeKind::User];
const USER_HOST_PROJECT_SHARED: &[ScopeKind] =
    &[ScopeKind::User, ScopeKind::Host, ScopeKind::ProjectShared];
const ALL_SCOPES: &[ScopeKind] = &[
    ScopeKind::User,
    ScopeKind::Host,
    ScopeKind::ProjectShared,
    ScopeKind::ProjectLocal,
];

static CATALOG: OnceLock<DefinitionCatalog> = OnceLock::new();

/// 定義データはプロセスで1つだけ構築する。
pub fn catalog() -> &'static DefinitionCatalog {
    CATALOG.get_or_init(build)
}

fn build() -> DefinitionCatalog {
    let catalog = DefinitionCatalog::new(categories(), pages(), settings());
    catalog
        .validate()
        .expect("設定定義データの整合性が保たれていません");
    catalog
}

fn categories() -> Vec<SettingCategory> {
    vec![
        SettingCategory {
            id: "appearance",
            label: "settings.category.appearance",
            groups: &[SettingGroup {
                id: "appearance.colors",
                label: "settings.group.appearance.colors",
                pages: &["appearance.theme", "appearance.syntax"],
            }],
        },
        SettingCategory {
            id: "terminal",
            label: "settings.category.terminal",
            groups: &[
                SettingGroup {
                    id: "terminal.presentation",
                    label: "settings.group.terminal.presentation",
                    pages: &["terminal.font"],
                },
                SettingGroup {
                    id: "terminal.launch",
                    label: "settings.group.terminal.startup",
                    pages: &["terminal.startup"],
                },
            ],
        },
        SettingCategory {
            id: "language",
            label: "settings.category.language",
            groups: &[SettingGroup {
                id: "language.display",
                label: "settings.group.language.display",
                pages: &["language.locale"],
            }],
        },
    ]
}

fn pages() -> Vec<SettingPage> {
    vec![
        SettingPage {
            id: "appearance.theme",
            category: "appearance",
            group: "appearance.colors",
            label: "settings.page.appearance.theme",
            description: Some("settings.page.appearance.theme.description"),
        },
        SettingPage {
            id: "appearance.syntax",
            category: "appearance",
            group: "appearance.colors",
            label: "settings.page.appearance.syntax",
            description: Some("settings.page.appearance.syntax.description"),
        },
        SettingPage {
            id: "terminal.font",
            category: "terminal",
            group: "terminal.presentation",
            label: "settings.page.terminal.font",
            description: None,
        },
        SettingPage {
            id: "terminal.startup",
            category: "terminal",
            group: "terminal.launch",
            label: "settings.page.terminal.startup",
            description: Some("settings.page.terminal.startup.description"),
        },
        SettingPage {
            id: "language.locale",
            category: "language",
            group: "language.display",
            label: "settings.page.language.locale",
            description: None,
        },
    ]
}

fn settings() -> Vec<SettingDefinition> {
    vec![
        SettingDefinition {
            key: UI_THEME,
            page: "appearance.theme",
            label: "settings.field.ui-theme",
            description: None,
            control: SettingControl::Choice(OptionSource::Themes),
            value_kind: ValueKind::Text,
            scopes: USER_HOST_PROJECT_SHARED,
            default: Some(SettingValue::text("lapis.dark")),
        },
        SettingDefinition {
            key: UI_SYNTAX_THEME,
            page: "appearance.syntax",
            label: "settings.field.ui-syntax-theme",
            description: Some("settings.field.ui-syntax-theme.description"),
            control: SettingControl::Choice(OptionSource::Values(syntax_options())),
            value_kind: ValueKind::Text,
            scopes: &[ScopeKind::User, ScopeKind::ProjectShared],
            default: Some(SettingValue::text(SyntaxTheme::Lapis.as_id())),
        },
        SettingDefinition {
            key: TERMINAL_FONT_FAMILY,
            page: "terminal.font",
            label: "settings.field.terminal-font",
            description: None,
            control: SettingControl::Choice(OptionSource::TerminalFonts),
            value_kind: ValueKind::Text,
            scopes: ALL_SCOPES,
            default: None,
        },
        SettingDefinition {
            key: TERMINAL_FONT_SIZE,
            page: "terminal.font",
            label: "settings.field.terminal-size",
            description: None,
            control: SettingControl::Choice(OptionSource::TerminalFontSizes),
            value_kind: ValueKind::Integer,
            scopes: ALL_SCOPES,
            default: None,
        },
        SettingDefinition {
            key: TERMINAL_LINE_HEIGHT,
            page: "terminal.font",
            label: "settings.field.terminal-line-height",
            description: None,
            control: SettingControl::Choice(OptionSource::TerminalLineHeights),
            value_kind: ValueKind::Integer,
            scopes: ALL_SCOPES,
            default: None,
        },
        SettingDefinition {
            key: TERMINAL_CARET_SHAPE,
            page: "terminal.font",
            label: "settings.field.terminal-caret",
            description: None,
            control: SettingControl::Choice(OptionSource::Values(caret_options())),
            value_kind: ValueKind::Text,
            scopes: ALL_SCOPES,
            default: None,
        },
        SettingDefinition {
            key: TERMINAL_DEFAULT_PROFILE,
            page: "terminal.startup",
            label: "settings.field.terminal-default-profile",
            description: Some("settings.field.terminal-default-profile.description"),
            control: SettingControl::Choice(OptionSource::TerminalProfiles),
            value_kind: ValueKind::Text,
            scopes: ALL_SCOPES,
            default: None,
        },
        SettingDefinition {
            key: UI_LOCALE,
            page: "language.locale",
            label: "settings.field.ui-locale",
            description: None,
            control: SettingControl::Choice(OptionSource::Locales),
            value_kind: ValueKind::Text,
            scopes: USER,
            default: Some(SettingValue::text("en-US")),
        },
    ]
}

fn syntax_options() -> Vec<SettingOption> {
    [
        (SyntaxTheme::Lapis, "Lapis"),
        (SyntaxTheme::VsCode, "VS Code"),
        (SyntaxTheme::Zed, "Zed"),
        (SyntaxTheme::Intellij, "IntelliJ (New)"),
        (SyntaxTheme::IntellijClassic, "IntelliJ (Classic / Darcula)"),
    ]
    .into_iter()
    .map(|(theme, name)| {
        SettingOption::with_label(SettingValue::text(theme.as_id()), Label::Literal(name))
    })
    .collect()
}

fn caret_options() -> Vec<SettingOption> {
    [
        (TerminalCaretShape::Block, "settings.terminal-caret.block"),
        (TerminalCaretShape::Bar, "settings.terminal-caret.bar"),
        (
            TerminalCaretShape::Underline,
            "settings.terminal-caret.underline",
        ),
    ]
    .into_iter()
    .map(|(shape, key)| {
        SettingOption::with_label(SettingValue::text(shape.as_id()), Label::Key(key))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        assert_eq!(catalog().validate(), Ok(()));
    }

    #[test]
    fn every_parent_row_is_classification_only() {
        let catalog = catalog();
        let page_ids = catalog
            .pages()
            .iter()
            .map(|page| page.id)
            .collect::<Vec<_>>();
        for definition in catalog.settings() {
            assert!(
                page_ids.contains(&definition.page),
                "設定 {} のページ {} は葉だけを指す",
                definition.key.as_str(),
                definition.page
            );
        }
        for category in catalog.categories() {
            assert!(!page_ids.contains(&category.id));
            for group in category.groups {
                assert!(!page_ids.contains(&group.id));
                for page in group.pages {
                    assert!(
                        catalog
                            .page(page)
                            .is_some_and(|entry| entry.group == group.id),
                        "中間層 {} のページ {} が一致しない",
                        group.id,
                        page
                    );
                }
            }
        }
    }

    #[test]
    fn terminal_options_match_saved_value_types() {
        for (key, kind) in [
            (TERMINAL_FONT_FAMILY, ValueKind::Text),
            (TERMINAL_FONT_SIZE, ValueKind::Integer),
            (TERMINAL_LINE_HEIGHT, ValueKind::Integer),
            (TERMINAL_CARET_SHAPE, ValueKind::Text),
            (TERMINAL_DEFAULT_PROFILE, ValueKind::Text),
        ] {
            let definition = catalog().definition(key).expect("定義がある");
            assert_eq!(definition.value_kind, kind);
            assert!(definition.default.is_none());
        }
    }

    #[test]
    fn scope_support_follows_the_issue_boundaries() {
        let scopes = |key| catalog().definition(key).unwrap().scopes;
        assert_eq!(scopes(UI_LOCALE), USER);
        assert_eq!(
            scopes(UI_SYNTAX_THEME),
            &[ScopeKind::User, ScopeKind::ProjectShared]
        );
        assert!(scopes(UI_THEME).contains(&ScopeKind::Host));
        assert!(!scopes(UI_THEME).contains(&ScopeKind::ProjectLocal));
        assert_eq!(scopes(TERMINAL_FONT_SIZE), ALL_SCOPES);
    }

    #[test]
    fn default_terminal_profile_is_a_project_overridable_choice() {
        let definition = catalog()
            .definition(TERMINAL_DEFAULT_PROFILE)
            .expect("既定 Terminal profile の定義がある");
        assert_eq!(
            definition.control,
            SettingControl::Choice(OptionSource::TerminalProfiles),
            "候補は Host が公開する profile から供給する"
        );
        assert_eq!(definition.page, "terminal.startup");
        assert_eq!(
            catalog().page("terminal.startup").map(|page| page.category),
            Some("terminal")
        );
    }
}
