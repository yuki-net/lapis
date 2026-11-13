//! 更新モードと配布元の設定値。
//!
//! 更新の実行そのものはアプリ側が所有する。ここは保存値の識別子と既定だけを決め、
//! 取得方法の実装を持たない（[配布と更新の内部契約](../../../../docs/specs/release.md)）。

/// 更新の扱い。軸A（通常更新）の適用頻度を決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMode {
    /// 裏で download・検証まで進め、次回起動で無言適用する。
    Auto,
    /// 新しい version を知らせるだけ。適用は利用者の操作から行う。
    Notify,
    /// 確認しない。
    Off,
}

impl UpdateMode {
    pub const fn as_id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Notify => "notify",
            Self::Off => "off",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "auto" => Some(Self::Auto),
            "notify" => Some(Self::Notify),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// 配布物の取得元。形式は共通で、入り口だけが違う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateSource {
    /// GitHub Releases の asset。
    Github,
    /// ローカル folder。
    Local,
    /// 自宅サーバーの静的配置。URL を location に持つ。
    Server,
}

impl UpdateSource {
    pub const fn as_id(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Local => "local",
            Self::Server => "server",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "github" => Some(Self::Github),
            "local" => Some(Self::Local),
            "server" => Some(Self::Server),
            _ => None,
        }
    }

    /// 取得元を切り替えると location の意味が変わる。切り替え時に以前の値を流用しない。
    pub const fn uses_location(self) -> bool {
        matches!(self, Self::Local | Self::Server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for mode in [UpdateMode::Auto, UpdateMode::Notify, UpdateMode::Off] {
            assert_eq!(UpdateMode::from_id(mode.as_id()), Some(mode));
        }
        for source in [
            UpdateSource::Github,
            UpdateSource::Local,
            UpdateSource::Server,
        ] {
            assert_eq!(UpdateSource::from_id(source.as_id()), Some(source));
        }
        assert_eq!(UpdateMode::from_id("Automatic"), None);
        assert_eq!(UpdateSource::from_id("ftp"), None);
    }

    #[test]
    fn only_folder_and_server_sources_need_a_location() {
        assert!(!UpdateSource::Github.uses_location());
        assert!(UpdateSource::Local.uses_location());
        assert!(UpdateSource::Server.uses_location());
    }
}
