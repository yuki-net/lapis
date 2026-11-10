//! 保存値と、定義データが持つ表示ラベル。

use serde::{Deserialize, Serialize};

/// 保存された設定値。定義の `ValueKind` と一致しなければならない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    Text(String),
    Integer(i64),
    Flag(bool),
}

impl SettingValue {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn integer(value: i64) -> Self {
        Self::Integer(value)
    }

    pub fn flag(value: bool) -> Self {
        Self::Flag(value)
    }

    pub fn kind(&self) -> ValueKind {
        match self {
            Self::Text(_) => ValueKind::Text,
            Self::Integer(_) => ValueKind::Integer,
            Self::Flag(_) => ValueKind::Flag,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            Self::Integer(_) | Self::Flag(_) => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            Self::Text(_) | Self::Flag(_) => None,
        }
    }
}

/// 保存値の種類。動的な候補でも型を検証できるように定義が明示する。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueKind {
    Text,
    Integer,
    Flag,
}

/// 表示ラベル。翻訳キーか、翻訳しない固定文字列。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Key(&'static str),
    Literal(&'static str),
}

/// 選択候補。`label` が無い場合は保存値をそのまま表示する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingOption {
    pub value: SettingValue,
    pub label: Option<Label>,
}

impl SettingOption {
    pub fn new(value: SettingValue) -> Self {
        Self { value, label: None }
    }

    pub fn with_label(value: SettingValue, label: Label) -> Self {
        Self {
            value,
            label: Some(label),
        }
    }
}
