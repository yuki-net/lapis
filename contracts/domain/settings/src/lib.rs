//! 設定の Scope / Owner 分離モデル。保存形式・実効値の解決・共有 state を提供する。
//!
//! - どの設定をどの所有者（User / Host / Project）が持てるかは [`definition`] の定義データが決める。
//! - 保存は所有者ごとの [`document::SettingsRepository`] が担う。
//! - UI は [`resolve::ResolvedSettings`] だけを読み、[`state::SettingsModel`] の購読で更新を受け取る。

pub mod catalog;
pub mod definition;
pub mod document;
pub mod error;
pub mod resolve;
pub mod scope;
pub mod syntax;
pub mod update;
pub mod value;

pub use catalog::catalog;
pub use definition::{
    DefinitionCatalog, OptionSource, SettingCategory, SettingControl, SettingDefinition,
    SettingGroup, SettingKey, SettingPage, SettingPageContent, TERMINAL_CARET_SHAPE,
    TERMINAL_DEFAULT_PROFILE, TERMINAL_FONT_FAMILY, TERMINAL_FONT_SIZE, TERMINAL_LINE_HEIGHT,
    UI_LOCALE, UI_SYNTAX_THEME, UI_THEME, UPDATE_MODE, UPDATE_SOURCE, UPDATE_SOURCE_LOCATION,
};
pub use document::{
    DOCUMENT_VERSION, LEGACY_DOCUMENT_VERSION, LegacyGlobalSettings, SettingsDocument,
    SettingsRepository,
};
pub use error::SettingsError;
pub use resolve::{ResolvedSetting, ResolvedSettings};
pub use scope::{HostId, ProjectId, ScopeKind, SettingsContext, SettingsScope};
pub use syntax::{SyntaxTheme, TerminalCaretShape};
pub use update::{UpdateMode, UpdateSource};
pub use value::{Label, SettingOption, SettingValue, ValueKind};

/// 既存の利用側が使う既定ロケール。定義データの既定値と一致させる。
pub const DEFAULT_LOCALE: &str = "en-US";
