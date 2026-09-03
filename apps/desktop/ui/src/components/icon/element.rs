use gpui::{IntoElement, Pixels, Styled, Svg, Transformation, radians, svg};

use super::{IconName, assets};

/// 固定アイコンを描画する GPUI 要素。
#[allow(dead_code)] // 既存画面の置換は行わず、段階的に利用する。
pub(crate) struct Icon {
    name: IconName,
    size: Option<Pixels>,
    transformation: Option<Transformation>,
}

impl Icon {
    #[allow(dead_code)] // 既存画面の置換は行わず、段階的に利用する。
    pub(crate) const fn new(name: IconName) -> Self {
        Self {
            name,
            size: None,
            transformation: None,
        }
    }

    pub(crate) fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }

    pub(crate) fn with_rotation(mut self, angle: f32) -> Self {
        self.transformation = Some(Transformation::rotate(radians(angle)));
        self
    }
}

impl IntoElement for Icon {
    type Element = Svg;

    fn into_element(self) -> Self::Element {
        // GPUI は SVG 自身に文字色がないと描画しない。SVG アセットや呼び出し側
        // には色を固定せず、この共通コンポーネントで有効テーマの色を解決する。
        let mut icon = svg()
            .path(assets::path(self.name))
            .text_color(crate::theme::colors().text_secondary);

        if let Some(size) = self.size {
            icon = icon.size(size);
        } else {
            icon = icon.size_4();
        }

        if let Some(transformation) = self.transformation {
            icon.with_transformation(transformation)
        } else {
            icon
        }
    }
}
