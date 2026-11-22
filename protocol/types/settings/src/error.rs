//! 設定の読み書きで返すエラー。

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(String);

impl SettingsError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SettingsError {}
