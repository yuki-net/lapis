use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

use super::IconName;

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

        Ok(None)
    }

    fn list(&self, directory: &str) -> gpui::Result<Vec<SharedString>> {
        if directory == "icons" {
            Ok(IconName::ALL
                .iter()
                .copied()
                .map(|name| path(name).trim_start_matches("icons/").into())
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
    }
}
