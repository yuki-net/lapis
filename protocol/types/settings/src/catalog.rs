//! アプリ設定の定義データ。画面とログイン設定の唯一の入力。

use std::sync::OnceLock;

use crate::definition::{
    DefinitionCatalog, OptionSource, SettingCategory, SettingControl, SettingDefinition,
    SettingGroup, SettingPage, SettingPageContent, TERMINAL_CARET_SHAPE, TERMINAL_DEFAULT_PROFILE,
    TERMINAL_FONT_FAMILY, TERMINAL_FONT_SIZE, TERMINAL_LINE_HEIGHT, UI_LOCALE, UI_SYNTAX_THEME,
    UI_THEME, UPDATE_MODE, UPDATE_SOURCE, UPDATE_SOURCE_LOCATION,
};
use crate::scope::ScopeKind;
use crate::syntax::{SyntaxTheme, TerminalCaretShape};
use crate::update::{UpdateMode, UpdateSource};
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

/// 現在の Web 接続情報を表示するページ。保存値を持たない。
pub const CONNECTION_WEB_PAGE: &str = "connection.web";

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
        // 接続先は保存値ではなく実行時の状態なので、カタログの葉として並べるが
        // 項目は持たず、画面側が現在の接続情報を表示する。
        SettingCategory {
            id: "connection",
            label: "settings.category.connection",
            groups: &[SettingGroup {
                id: "connection.local",
                label: "settings.group.connection.web",
                pages: &["connection.web"],
            }],
        },
        SettingCategory {
            id: "update",
            label: "settings.category.update",
            groups: &[SettingGroup {
                id: "update.install",
                label: "settings.group.update.install",
                pages: &["update.channel"],
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
            content: SettingPageContent::Settings,
        },
        SettingPage {
            id: "appearance.syntax",
            category: "appearance",
            group: "appearance.colors",
            label: "settings.page.appearance.syntax",
            description: Some("settings.page.appearance.syntax.description"),
            content: SettingPageContent::Settings,
        },
        SettingPage {
            id: "terminal.font",
            category: "terminal",
            group: "terminal.presentation",
            label: "settings.page.terminal.font",
            description: None,
            content: SettingPageContent::Settings,
        },
        SettingPage {
            id: "terminal.startup",
            category: "terminal",
            group: "terminal.launch",
            label: "settings.page.terminal.startup",
            description: Some("settings.page.terminal.startup.description"),
            content: SettingPageContent::Settings,
        },
        SettingPage {
            id: "language.locale",
            category: "language",
            group: "language.display",
            label: "settings.page.language.locale",
            description: None,
            content: SettingPageContent::Settings,
        },
        SettingPage {
            id: "connection.web",
            category: "connection",
            group: "connection.local",
            label: "settings.page.connection.web",
            description: Some("settings.page.connection.web.description"),
            content: SettingPageContent::Feature,
        },
        SettingPage {
            id: "update.channel",
            category: "update",
            group: "update.install",
            label: "settings.page.update.channel",
            description: Some("settings.page.update.channel.description"),
            content: SettingPageContent::Settings,
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
            control: SettingControl::Choice(OptionSource::Provided),
            value_kind: ValueKind::Text,
            scopes: ALL_SCOPES,
            default: None,
        },
        SettingDefinition {
            key: TERMINAL_FONT_SIZE,
            page: "terminal.font",
            label: "settings.field.terminal-size",
            description: None,
            control: SettingControl::Choice(OptionSource::Provided),
            value_kind: ValueKind::Integer,
            scopes: ALL_SCOPES,
            default: None,
        },
        SettingDefinition {
            key: TERMINAL_LINE_HEIGHT,
            page: "terminal.font",
            label: "settings.field.terminal-line-height",
            description: None,
            control: SettingControl::Choice(OptionSource::Provided),
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
            control: SettingControl::Choice(OptionSource::Provided),
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
        SettingDefinition {
            key: UPDATE_MODE,
            page: "update.channel",
            label: "settings.field.update-mode",
            description: Some("settings.field.update-mode.description"),
            control: SettingControl::Choice(OptionSource::Values(update_mode_options())),
            value_kind: ValueKind::Text,
            scopes: USER,
            default: Some(SettingValue::text(UpdateMode::Auto.as_id())),
        },
        SettingDefinition {
            key: UPDATE_SOURCE,
            page: "update.channel",
            label: "settings.field.update-source",
            description: Some("settings.field.update-source.description"),
            control: SettingControl::Choice(OptionSource::Values(update_source_options())),
            value_kind: ValueKind::Text,
            scopes: USER,
            default: Some(SettingValue::text(UpdateSource::Github.as_id())),
        },
        SettingDefinition {
            key: UPDATE_SOURCE_LOCATION,
            page: "update.channel",
            label: "settings.field.update-source-location",
            description: Some("settings.field.update-source-location.description"),
            control: SettingControl::Text,
            value_kind: ValueKind::Text,
            scopes: USER,
            default: None,
        },
    ]
}

fn update_mode_options() -> Vec<SettingOption> {
    [
        (UpdateMode::Auto, "settings.update-mode.auto"),
        (UpdateMode::Notify, "settings.update-mode.notify"),
        (UpdateMode::Off, "settings.update-mode.off"),
    ]
    .into_iter()
    .map(|(mode, key)| SettingOption::with_label(SettingValue::text(mode.as_id()), Label::Key(key)))
    .collect()
}

fn update_source_options() -> Vec<SettingOption> {
    [
        (UpdateSource::Github, "settings.update-source.github"),
        (UpdateSource::Local, "settings.update-source.local"),
        (UpdateSource::Server, "settings.update-source.server"),
    ]
    .into_iter()
    .map(|(source, key)| {
        SettingOption::with_label(SettingValue::text(source.as_id()), Label::Key(key))
    })
    .collect()
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
    fn every_leaf_page_is_reachable_from_the_navigation() {
        let catalog = catalog();
        let mut reachable = catalog
            .categories()
            .iter()
            .flat_map(|category| category.groups.iter())
            .flat_map(|group| group.pages.iter())
            .copied()
            .collect::<Vec<_>>();
        reachable.sort_unstable();
        let mut pages = catalog
            .pages()
            .iter()
            .map(|page| page.id)
            .collect::<Vec<_>>();
        pages.sort_unstable();
        assert_eq!(reachable, pages);
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
            SettingControl::Choice(OptionSource::Provided),
            "候補は Host が公開する profile から供給する"
        );
        assert_eq!(definition.page, "terminal.startup");
        assert_eq!(
            catalog().page("terminal.startup").map(|page| page.category),
            Some("terminal")
        );
    }

    #[test]
    fn update_settings_are_user_only_choices_with_defaults() {
        for (key, expected) in [
            (UPDATE_MODE, UpdateMode::Auto.as_id()),
            (UPDATE_SOURCE, UpdateSource::Github.as_id()),
        ] {
            let definition = catalog().definition(key).expect("更新設定の定義がある");
            assert_eq!(definition.scopes, USER);
            assert_eq!(definition.page, "update.channel");
            assert_eq!(
                definition.default.as_ref().and_then(SettingValue::as_text),
                Some(expected)
            );
        }
        // 配布元の location は入力候補を持たず、取得元ごとに意味が変わる。
        let location = catalog()
            .definition(UPDATE_SOURCE_LOCATION)
            .expect("location の定義がある");
        assert!(location.default.is_none());
        assert_eq!(location.control, SettingControl::Text);
        assert_eq!(location.page, "update.channel");
    }

    #[test]
    fn update_options_name_every_stored_value() {
        let options = |key| {
            let definition = catalog().definition(key).expect("定義がある");
            let SettingControl::Choice(OptionSource::Values(values)) = &definition.control else {
                panic!("固定候補の選択肢である");
            };
            values
                .iter()
                .filter_map(|option| option.value.as_text().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            options(UPDATE_MODE),
            ["auto", "notify", "off"].map(ToOwned::to_owned)
        );
        assert_eq!(
            options(UPDATE_SOURCE),
            ["github", "local", "server"].map(ToOwned::to_owned)
        );
    }

    /// 接続ページは保存値を持たないが、ナビの葉として到達できる必要がある。
    #[test]
    fn connection_page_is_a_feature_leaf_without_settings() {
        let catalog = catalog();
        let page = catalog
            .page(CONNECTION_WEB_PAGE)
            .expect("接続ページの定義がある");
        assert_eq!(page.content, SettingPageContent::Feature);
        assert_eq!(page.category, "connection");
        assert_eq!(page.group, "connection.local");
        assert_eq!(catalog.settings_of(CONNECTION_WEB_PAGE).count(), 0);
        assert!(
            catalog
                .categories()
                .iter()
                .flat_map(|category| category.groups.iter())
                .any(|group| group.pages.contains(&CONNECTION_WEB_PAGE)),
            "接続ページは分類ツリーの葉として並ぶ"
        );
    }
}
