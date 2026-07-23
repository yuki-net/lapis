use gpui::{IntoElement, Styled, Svg, svg};

use super::{IconName, assets};

/// 固定アイコンを描画する GPUI 要素。
#[allow(dead_code)] // 既存画面の置換は行わず、段階的に利用する。
pub(crate) struct Icon {
    name: IconName,
}

impl Icon {
    #[allow(dead_code)] // 既存画面の置換は行わず、段階的に利用する。
    pub(crate) const fn new(name: IconName) -> Self {
        Self { name }
    }
}

impl IntoElement for Icon {
    type Element = Svg;

    fn into_element(self) -> Self::Element {
        svg().path(assets::path(self.name)).size_4()
    }
}
