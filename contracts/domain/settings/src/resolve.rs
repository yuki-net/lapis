//! スコープをまたいだ実効値の解決。

use std::collections::BTreeMap;

use localization::LocaleId;

use crate::definition::{
    DefinitionCatalog, SettingKey, TERMINAL_CARET_SHAPE, TERMINAL_DEFAULT_PROFILE,
    TERMINAL_FONT_FAMILY, TERMINAL_FONT_SIZE, TERMINAL_LINE_HEIGHT, UI_LOCALE, UI_SYNTAX_THEME,
    UI_THEME, UPDATE_MODE, UPDATE_SOURCE, UPDATE_SOURCE_LOCATION,
};
use crate::document::SettingsDocument;
use crate::scope::{SettingsContext, SettingsScope};
use crate::syntax::{SyntaxTheme, TerminalCaretShape};
use crate::update::{UpdateMode, UpdateSource};
use crate::value::SettingValue;

/// 1項目の解決結果。値が無い場合は `value` も `origin` も空になる。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSetting {
    value: Option<SettingValue>,
    origin: Option<SettingsScope>,
}

impl ResolvedSetting {
    pub fn value(&self) -> Option<&SettingValue> {
        self.value.as_ref()
    }

    /// 値を保存しているスコープ。既定値のときは `None`。
    pub fn origin(&self) -> Option<&SettingsScope> {
        self.origin.as_ref()
    }
}

/// コンテキストに対する実効値一式。UI はこの型だけを読む。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSettings {
    context: SettingsContext,
    settings: BTreeMap<SettingKey, ResolvedSetting>,
}

impl ResolvedSettings {
    /// 参照可能な文書だけを対象に、優先順位の高いスコープで上書きしながら解決する。
    pub fn resolve(
        context: &SettingsContext,
        catalog: &'static DefinitionCatalog,
        documents: &[&SettingsDocument],
    ) -> Self {
        let scopes = context.scopes();
        let mut settings = BTreeMap::new();
        for definition in catalog.settings() {
            let mut resolved = ResolvedSetting {
                value: definition.default.clone(),
                origin: None,
            };
            for scope in &scopes {
                let Some(document) = documents.iter().find(|document| document.scope() == scope)
                else {
                    continue;
                };
                if let Some(value) = document.get(definition.key) {
                    resolved = ResolvedSetting {
                        value: Some(value.clone()),
                        origin: Some(scope.clone()),
                    };
                }
            }
            settings.insert(definition.key, resolved);
        }
        Self {
            context: context.clone(),
            settings,
        }
    }

    pub fn context(&self) -> &SettingsContext {
        &self.context
    }

    pub fn setting(&self, key: SettingKey) -> Option<&ResolvedSetting> {
        self.settings.get(&key)
    }

    pub fn value(&self, key: SettingKey) -> Option<&SettingValue> {
        self.setting(key).and_then(ResolvedSetting::value)
    }

    pub fn origin(&self, key: SettingKey) -> Option<&SettingsScope> {
        self.setting(key).and_then(ResolvedSetting::origin)
    }

    pub fn text(&self, key: SettingKey) -> Option<&str> {
        self.value(key).and_then(SettingValue::as_text)
    }

    pub fn integer(&self, key: SettingKey) -> Option<i64> {
        self.value(key).and_then(SettingValue::as_integer)
    }

    /// 保存前の値を一時的に上書きする。楽観更新の表示に使う。
    pub fn overlay(&mut self, scope: SettingsScope, key: SettingKey, value: SettingValue) {
        self.settings.insert(
            key,
            ResolvedSetting {
                value: Some(value),
                origin: Some(scope),
            },
        );
    }

    /// 上書きを取り消し、表示を既定値へ戻す。保存値は変更しない。
    pub fn clear_overlay(&mut self, key: SettingKey) {
        if let Some(resolved) = self.settings.get_mut(&key) {
            resolved.value = None;
            resolved.origin = None;
        }
    }

    pub fn theme(&self) -> &str {
        self.text(UI_THEME).unwrap_or("lapis.dark")
    }

    pub fn locale(&self) -> LocaleId {
        LocaleId::new(self.text(UI_LOCALE).unwrap_or("en-US"))
    }

    pub fn syntax_theme(&self) -> Option<SyntaxTheme> {
        self.text(UI_SYNTAX_THEME).and_then(SyntaxTheme::from_id)
    }

    pub fn terminal_font_family(&self) -> Option<String> {
        self.text(TERMINAL_FONT_FAMILY).map(ToOwned::to_owned)
    }

    pub fn terminal_font_size(&self) -> Option<u8> {
        self.u8_setting(TERMINAL_FONT_SIZE)
    }

    pub fn terminal_line_height_percent(&self) -> Option<u8> {
        self.u8_setting(TERMINAL_LINE_HEIGHT)
    }

    pub fn terminal_caret_shape(&self) -> Option<TerminalCaretShape> {
        self.text(TERMINAL_CARET_SHAPE)
            .and_then(TerminalCaretShape::from_id)
    }

    /// 設定で選ばれた既定 Terminal profile。未設定なら Host の既定を使う。
    pub fn terminal_default_profile(&self) -> Option<String> {
        self.text(TERMINAL_DEFAULT_PROFILE)
            .filter(|id| !id.is_empty())
            .map(ToOwned::to_owned)
    }

    /// 更新モード。未設定・不正値は既定（自動）へ倒す。
    pub fn update_mode(&self) -> UpdateMode {
        self.text(UPDATE_MODE)
            .and_then(UpdateMode::from_id)
            .unwrap_or(UpdateMode::Auto)
    }

    /// 配布元。未設定・不正値は既定（GitHub Releases）へ倒す。
    pub fn update_source(&self) -> UpdateSource {
        self.text(UPDATE_SOURCE)
            .and_then(UpdateSource::from_id)
            .unwrap_or(UpdateSource::Github)
    }

    /// 配布元の location。local は folder の path、server は URL。
    pub fn update_source_location(&self) -> Option<String> {
        self.text(UPDATE_SOURCE_LOCATION)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    fn u8_setting(&self, key: SettingKey) -> Option<u8> {
        self.integer(key).and_then(|value| u8::try_from(value).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog;
    use crate::scope::{HostId, ProjectId};

    fn document(scope: SettingsScope, values: &[(SettingKey, SettingValue)]) -> SettingsDocument {
        let mut document = SettingsDocument::empty(scope);
        for (key, value) in values {
            document.insert(*key, value.clone());
        }
        document
    }

    #[test]
    fn deeper_scopes_override_user_values() {
        let context = SettingsContext::new()
            .with_host(HostId::new("host-a"))
            .with_project(ProjectId::new("lapis"));
        let user = document(
            SettingsScope::User,
            &[
                (UI_THEME, SettingValue::text("lapis.dark")),
                (TERMINAL_FONT_SIZE, SettingValue::integer(13)),
            ],
        );
        let host = document(
            SettingsScope::Host {
                id: HostId::new("host-a"),
            },
            &[(TERMINAL_FONT_SIZE, SettingValue::integer(15))],
        );
        let local = document(
            SettingsScope::ProjectLocal {
                id: ProjectId::new("lapis"),
            },
            &[(TERMINAL_FONT_SIZE, SettingValue::integer(17))],
        );
        let resolved = ResolvedSettings::resolve(&context, catalog(), &[&user, &host, &local]);
        assert_eq!(resolved.terminal_font_size(), Some(17));
        assert_eq!(
            resolved.origin(TERMINAL_FONT_SIZE),
            Some(&SettingsScope::ProjectLocal {
                id: ProjectId::new("lapis")
            })
        );
        assert_eq!(resolved.theme(), "lapis.dark");
        assert_eq!(resolved.origin(UI_THEME), Some(&SettingsScope::User));
    }

    #[test]
    fn definitions_supply_defaults_without_an_origin() {
        let resolved = ResolvedSettings::resolve(&SettingsContext::new(), catalog(), &[]);
        assert_eq!(resolved.theme(), "lapis.dark");
        assert_eq!(resolved.origin(UI_THEME), None);
        assert_eq!(resolved.locale().as_str(), "en-US");
        assert_eq!(resolved.syntax_theme(), Some(SyntaxTheme::Lapis));
        assert_eq!(resolved.terminal_font_family(), None);
        assert_eq!(resolved.terminal_caret_shape(), None);
    }

    #[test]
    fn documents_outside_the_context_are_ignored() {
        let context = SettingsContext::new();
        let shared = document(
            SettingsScope::ProjectShared {
                id: ProjectId::new("lapis"),
            },
            &[(TERMINAL_FONT_SIZE, SettingValue::integer(20))],
        );
        let resolved = ResolvedSettings::resolve(&context, catalog(), &[&shared]);
        assert_eq!(resolved.terminal_font_size(), None);
    }

    #[test]
    fn overlay_wins_until_the_saved_value_arrives() {
        let user = document(
            SettingsScope::User,
            &[(TERMINAL_FONT_SIZE, SettingValue::integer(13))],
        );
        let mut resolved = ResolvedSettings::resolve(&SettingsContext::new(), catalog(), &[&user]);
        resolved.overlay(
            SettingsScope::User,
            TERMINAL_FONT_SIZE,
            SettingValue::integer(18),
        );
        assert_eq!(resolved.terminal_font_size(), Some(18));
    }

    #[test]
    fn terminal_default_profile_is_empty_until_it_is_chosen() {
        let resolved = ResolvedSettings::resolve(&SettingsContext::new(), catalog(), &[]);
        assert_eq!(resolved.terminal_default_profile(), None);
        let user = document(
            SettingsScope::User,
            &[(TERMINAL_DEFAULT_PROFILE, SettingValue::text(""))],
        );
        let resolved = ResolvedSettings::resolve(&SettingsContext::new(), catalog(), &[&user]);
        assert_eq!(
            resolved.terminal_default_profile(),
            None,
            "空文字は未設定として扱う"
        );
        let user = document(
            SettingsScope::User,
            &[(TERMINAL_DEFAULT_PROFILE, SettingValue::text("cmd"))],
        );
        let resolved = ResolvedSettings::resolve(&SettingsContext::new(), catalog(), &[&user]);
        assert_eq!(resolved.terminal_default_profile().as_deref(), Some("cmd"));
    }

    #[test]
    fn update_settings_default_to_auto_from_github_and_reject_unknown_values() {
        let resolved = ResolvedSettings::resolve(&SettingsContext::new(), catalog(), &[]);
        assert_eq!(resolved.update_mode(), UpdateMode::Auto);
        assert_eq!(resolved.update_source(), UpdateSource::Github);
        assert_eq!(resolved.update_source_location(), None);

        let user = document(
            SettingsScope::User,
            &[
                (UPDATE_MODE, SettingValue::text("notify")),
                (UPDATE_SOURCE, SettingValue::text("server")),
                (
                    UPDATE_SOURCE_LOCATION,
                    SettingValue::text("https://updates.example.invalid/lapis"),
                ),
            ],
        );
        let resolved = ResolvedSettings::resolve(&SettingsContext::new(), catalog(), &[&user]);
        assert_eq!(resolved.update_mode(), UpdateMode::Notify);
        assert_eq!(resolved.update_source(), UpdateSource::Server);
        assert_eq!(
            resolved.update_source_location().as_deref(),
            Some("https://updates.example.invalid/lapis")
        );

        // 保存値が壊れていても起動を止めず、既定へ倒す。
        let user = document(
            SettingsScope::User,
            &[
                (UPDATE_MODE, SettingValue::text("sometimes")),
                (UPDATE_SOURCE, SettingValue::text("ftp")),
                (UPDATE_SOURCE_LOCATION, SettingValue::text("  ")),
            ],
        );
        let resolved = ResolvedSettings::resolve(&SettingsContext::new(), catalog(), &[&user]);
        assert_eq!(resolved.update_mode(), UpdateMode::Auto);
        assert_eq!(resolved.update_source(), UpdateSource::Github);
        assert_eq!(resolved.update_source_location(), None);
    }
}
