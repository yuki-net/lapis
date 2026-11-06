use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

use super::{IconName, file_catalog::FileIconId};

pub(super) const fn path(name: IconName) -> &'static str {
    name.asset().path
}

/// コンパイル時に埋め込んだ固定アイコンを GPUI に供給する。
pub(crate) struct IconAssets;

impl AssetSource for IconAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        for &name in IconName::ALL {
            let asset = name.asset();
            if asset.path == path {
                return Ok(Some(Cow::Borrowed(asset.bytes)));
            }
        }

        for &name in FileIconId::ALL {
            let asset = name.asset();
            if asset.path == path {
                return Ok(Some(Cow::Borrowed(asset.bytes)));
            }
        }

        Ok(None)
    }

    fn list(&self, directory: &str) -> gpui::Result<Vec<SharedString>> {
        if directory == "icons" {
            Ok(IconName::ALL
                .iter()
                .copied()
                .map(|name| path(name).trim_start_matches("icons/").into())
                .collect())
        } else if directory == "file-icons" {
            Ok(FileIconId::ALL
                .iter()
                .copied()
                .map(|name| name.asset().path.trim_start_matches("file-icons/").into())
                .collect())
        } else {
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_icon_has_a_bundled_asset() {
        assert_eq!(path(IconName::Menu), "icons/menu.svg");
        assert_eq!(path(IconName::Search), "icons/search.svg");
        assert_eq!(path(IconName::Close), "icons/close.svg");
        assert_eq!(path(IconName::Minus), "icons/minus.svg");
        assert_eq!(path(IconName::Square), "icons/square.svg");
        assert_eq!(path(IconName::X), "icons/x.svg");
        assert_eq!(path(IconName::PanelLeft), "icons/panel-left.svg");
        assert_eq!(
            path(IconName::PanelLeftDashed),
            "icons/panel-left-dashed.svg"
        );
        assert_eq!(path(IconName::Settings), "icons/settings.svg");
        assert_eq!(path(IconName::SunMoon), "icons/sun-moon.svg");
    }
}
