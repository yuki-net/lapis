//! 設定の定義データ。UI はこのデータだけを入力に画面を組み立てる。

use crate::scope::{ScopeKind, SettingsScope};
use crate::value::{Label, SettingOption, SettingValue, ValueKind};

/// 設定キー。定義カタログに存在するキーだけが保存できる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SettingKey(&'static str);

impl SettingKey {
    pub const fn new(key: &'static str) -> Self {
        Self(key)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

pub const UI_THEME: SettingKey = SettingKey::new("ui.theme");
pub const UI_SYNTAX_THEME: SettingKey = SettingKey::new("ui.syntax-theme");
pub const UI_LOCALE: SettingKey = SettingKey::new("ui.locale");
pub const TERMINAL_FONT_FAMILY: SettingKey = SettingKey::new("terminal.font-family");
pub const TERMINAL_FONT_SIZE: SettingKey = SettingKey::new("terminal.font-size");
pub const TERMINAL_LINE_HEIGHT: SettingKey = SettingKey::new("terminal.line-height-percent");
pub const TERMINAL_CARET_SHAPE: SettingKey = SettingKey::new("terminal.caret-shape");
pub const TERMINAL_DEFAULT_PROFILE: SettingKey = SettingKey::new("terminal.default-profile");
pub const UPDATE_MODE: SettingKey = SettingKey::new("update.mode");
pub const UPDATE_SOURCE: SettingKey = SettingKey::new("update.source");
pub const UPDATE_SOURCE_LOCATION: SettingKey = SettingKey::new("update.source-location");

/// 選択候補の供給元。値そのものではなく、UI が所有する一覧を指す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionSource {
    /// 定義データが持つ固定候補。
    Values(Vec<SettingOption>),
    /// 登録済みのUIテーマ。
    Themes,
    /// 対応ロケール。
    Locales,
    /// ターミナルの等幅フォント。
    TerminalFonts,
    /// ターミナルのフォントサイズ。
    TerminalFontSizes,
    /// ターミナルの行送り。
    TerminalLineHeights,
    /// Host が公開するターミナル profile。
    TerminalProfiles,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingControl {
    Choice(OptionSource),
    /// 単一行のテキスト入力。候補を持たない値（path や URL）に使う。
    Text,
}

/// 1つの設定項目の定義。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingDefinition {
    pub key: SettingKey,
    /// 項目を表示するページID。
    pub page: &'static str,
    /// 表示名の翻訳キー。
    pub label: &'static str,
    /// 補足説明の翻訳キー。
    pub description: Option<&'static str>,
    pub control: SettingControl,
    pub value_kind: ValueKind,
    /// どのスコープまで保存を許すか。定義が無いスコープは保存できない。
    pub scopes: &'static [ScopeKind],
    /// どこにも保存されていないときの値。
    pub default: Option<SettingValue>,
}

impl SettingDefinition {
    pub fn accepts(&self, value: &SettingValue) -> bool {
        if value.kind() != self.value_kind {
            return false;
        }
        match &self.control {
            SettingControl::Choice(OptionSource::Values(options)) => {
                options.iter().any(|option| &option.value == value)
            }
            SettingControl::Choice(_) => true,
            SettingControl::Text => value.as_text().is_some_and(|text| !text.trim().is_empty()),
        }
    }

    pub fn allows_scope(&self, scope: &SettingsScope) -> bool {
        self.scopes.contains(&scope.kind())
    }
}

/// ナビゲーション最上位の分類。子を持つだけで、自分自身は設定を持たない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingCategory {
    pub id: &'static str,
    pub label: &'static str,
    pub groups: &'static [SettingGroup],
}

/// 分類の下に置く中間層。これも分類専用で、設定を持たない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingGroup {
    pub id: &'static str,
    pub label: &'static str,
    pub pages: &'static [&'static str],
}

/// 実際に設定項目を並べるページ。1ページに1つ以上の項目を持つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingPage {
    pub id: &'static str,
    pub category: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub description: Option<&'static str>,
}

/// 定義データ一式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinitionCatalog {
    categories: Vec<SettingCategory>,
    pages: Vec<SettingPage>,
    settings: Vec<SettingDefinition>,
}

impl DefinitionCatalog {
    pub fn new(
        categories: Vec<SettingCategory>,
        pages: Vec<SettingPage>,
        settings: Vec<SettingDefinition>,
    ) -> Self {
        Self {
            categories,
            pages,
            settings,
        }
    }

    pub fn categories(&self) -> &[SettingCategory] {
        &self.categories
    }

    pub fn pages(&self) -> &[SettingPage] {
        &self.pages
    }

    pub fn settings(&self) -> &[SettingDefinition] {
        &self.settings
    }

    pub fn page(&self, id: &str) -> Option<&SettingPage> {
        self.pages.iter().find(|page| page.id == id)
    }

    pub fn definition(&self, key: SettingKey) -> Option<&SettingDefinition> {
        self.settings
            .iter()
            .find(|definition| definition.key == key)
    }

    /// ページに並べる項目を定義順で返す。
    pub fn settings_of(&self, page: &str) -> impl Iterator<Item = &SettingDefinition> {
        self.settings
            .iter()
            .filter(move |definition| definition.page == page)
    }

    pub fn first_page(&self) -> Option<&SettingPage> {
        self.categories
            .iter()
            .flat_map(|category| category.groups.iter())
            .find_map(|group| group.pages.first())
            .and_then(|page| self.page(page))
    }

    /// ナビゲーションが壊れていないかを検証する。
    pub fn validate(&self) -> Result<(), String> {
        for category in &self.categories {
            if self
                .categories
                .iter()
                .filter(|other| other.id == category.id)
                .count()
                > 1
            {
                return Err(format!("分類IDが重複しています: {}", category.id));
            }
            if category.groups.is_empty() {
                return Err(format!("分類にページがありません: {}", category.id));
            }
        }
        for group in self.categories.iter().flat_map(|c| c.groups.iter()) {
            if group.pages.is_empty() {
                return Err(format!("中間層にページがありません: {}", group.id));
            }
        }
        for page in &self.pages {
            if self.pages.iter().filter(|p| p.id == page.id).count() > 1 {
                return Err(format!("ページIDが重複しています: {}", page.id));
            }
            let owned = self.categories.iter().any(|category| {
                category.id == page.category
                    && category
                        .groups
                        .iter()
                        .any(|group| group.id == page.group && group.pages.contains(&page.id))
            });
            if !owned {
                return Err(format!("ページが分類ツリーに属していません: {}", page.id));
            }
            if self.settings_of(page.id).next().is_none() {
                return Err(format!("ページに設定項目がありません: {}", page.id));
            }
        }
        for definition in &self.settings {
            if self
                .settings
                .iter()
                .filter(|other| other.key == definition.key)
                .count()
                > 1
            {
                return Err(format!(
                    "設定キーが重複しています: {}",
                    definition.key.as_str()
                ));
            }
            if self.page(definition.page).is_none() {
                return Err(format!(
                    "設定のページが存在しません: {} -> {}",
                    definition.key.as_str(),
                    definition.page
                ));
            }
            if definition.scopes.is_empty() {
                return Err(format!(
                    "設定にスコープがありません: {}",
                    definition.key.as_str()
                ));
            }
            if let Some(default) = &definition.default
                && !definition.accepts(default)
            {
                return Err(format!(
                    "既定値が定義と一致しません: {}",
                    definition.key.as_str()
                ));
            }
        }
        Ok(())
    }
}

/// 表示ラベルの翻訳キーを取り出す。
pub fn label_key(label: Label) -> Option<&'static str> {
    match label {
        Label::Key(key) => Some(key),
        Label::Literal(_) => None,
    }
}
