use super::*;
use localization::LocaleId;
use settings::{
    DefinitionCatalog, ResolvedSettings, SettingKey, SettingValue, SettingsContext, SettingsError,
    SettingsRepository, SettingsScope,
};
use std::sync::Arc;

/// 設定値を UI に公開し、変更を所有者ごとの保存先へ永続化するアプリケーション境界。
#[derive(Clone)]
pub struct SettingsSession {
    model: Arc<SettingsModel>,
    scope: SettingsScope,
}

impl SettingsSession {
    /// Host 接続ごとに独立した宣言の受理先を作る。
    pub fn extension_contributions(&self) -> ExtensionContributionSession {
        ExtensionContributionSession::new(Arc::clone(&self.model))
    }
    /// ユーザースコープだけを持つ Session を作る。
    pub fn load(repository: Arc<dyn SettingsRepository>) -> Result<Self, SettingsError> {
        Self::for_scope(SettingsContext::default(), SettingsScope::User, repository)
    }

    /// 解決コンテキストと、既定の書き込み先スコープを指定して Session を作る。
    pub fn for_scope(
        context: SettingsContext,
        scope: SettingsScope,
        repository: Arc<dyn SettingsRepository>,
    ) -> Result<Self, SettingsError> {
        let model = SettingsModel::new(context);
        model.attach(scope.clone(), repository)?;
        Ok(Self {
            model: Arc::new(model),
            scope,
        })
    }

    /// 追加のスコープ（Host / Project）を解決対象に加える。
    pub fn attach(
        &self,
        scope: SettingsScope,
        repository: Arc<dyn SettingsRepository>,
    ) -> Result<(), SettingsError> {
        self.model.attach(scope, repository)
    }

    /// 現在のコンテキストに対する実効値。
    pub fn settings(&self) -> ResolvedSettings {
        self.model.resolved()
    }

    pub fn catalog(&self) -> &'static DefinitionCatalog {
        settings::catalog()
    }

    pub fn scope(&self) -> &SettingsScope {
        &self.scope
    }

    /// 指定スコープへ保存する。
    pub fn set_in(
        &self,
        scope: SettingsScope,
        key: SettingKey,
        value: SettingValue,
    ) -> Result<(), SettingsError> {
        self.model.set(scope, key, value)
    }

    /// 複数の設定を1回の保存でまとめて更新する。`None` は既定値へ戻す。
    pub fn update(
        &self,
        changes: &[(SettingKey, Option<SettingValue>)],
    ) -> Result<(), SettingsError> {
        self.model.update(self.scope.clone(), changes)
    }

    pub fn set(&self, key: SettingKey, value: SettingValue) -> Result<(), SettingsError> {
        self.model.set(self.scope.clone(), key, value)
    }

    pub fn clear(&self, key: SettingKey) -> Result<(), SettingsError> {
        self.model.clear(self.scope.clone(), key)
    }

    /// 設定変更の購読を開始する。
    pub fn subscribe(&self) -> SettingsWatch {
        self.model.subscribe()
    }

    pub fn set_locale(&self, locale: LocaleId) -> Result<(), SettingsError> {
        self.set(settings::UI_LOCALE, SettingValue::text(locale.as_str()))
    }

    pub fn set_theme(&self, theme: String) -> Result<(), SettingsError> {
        self.set(settings::UI_THEME, SettingValue::text(theme))
    }

    /// 構文配色を保存する。`None` は既定値へ戻す。
    pub fn set_syntax_theme(
        &self,
        theme: Option<settings::SyntaxTheme>,
    ) -> Result<(), SettingsError> {
        match theme {
            Some(theme) => self.set(settings::UI_SYNTAX_THEME, SettingValue::text(theme.as_id())),
            None => self.clear(settings::UI_SYNTAX_THEME),
        }
    }

    /// ターミナルの表示設定をまとめて保存する。`None` は既定値へ戻す。
    pub fn set_terminal_appearance(
        &self,
        family: Option<String>,
        size: Option<u8>,
        line_height_percent: Option<u8>,
        caret_shape: Option<settings::TerminalCaretShape>,
    ) -> Result<(), SettingsError> {
        self.update(&[
            (
                settings::TERMINAL_FONT_FAMILY,
                family.map(SettingValue::text),
            ),
            (
                settings::TERMINAL_FONT_SIZE,
                size.map(|value| SettingValue::integer(i64::from(value))),
            ),
            (
                settings::TERMINAL_LINE_HEIGHT,
                line_height_percent.map(|value| SettingValue::integer(i64::from(value))),
            ),
            (
                settings::TERMINAL_CARET_SHAPE,
                caret_shape.map(|shape| SettingValue::text(shape.as_id())),
            ),
        ])
    }
}
