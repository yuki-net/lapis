use gpui::{IntoElement, Pixels, Styled, Svg, svg};

use super::{FileIconId, file_catalog};

pub(crate) struct FileIcon {
    name: FileIconId,
    size: Option<Pixels>,
}

impl FileIcon {
    pub(crate) const fn new(name: FileIconId) -> Self {
        Self { name, size: None }
    }

    pub(crate) fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }
}

impl IntoElement for FileIcon {
    type Element = Svg;

    fn into_element(self) -> Self::Element {
        let mut icon = svg()
            .path(file_catalog::FileIconId::asset(self.name).path)
            .text_color(crate::theme::colors().text_secondary);

        if let Some(size) = self.size {
            icon = icon.size(size);
        } else {
            icon = icon.size_4();
        }

        icon
    }
}
