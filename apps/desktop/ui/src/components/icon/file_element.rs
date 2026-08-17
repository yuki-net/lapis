use gpui::{IntoElement, Styled, Svg, svg};

use super::{FileIconId, file_catalog};

pub(crate) struct FileIcon {
    name: FileIconId,
}

impl FileIcon {
    pub(crate) const fn new(name: FileIconId) -> Self {
        Self { name }
    }
}

impl IntoElement for FileIcon {
    type Element = Svg;

    fn into_element(self) -> Self::Element {
        svg()
            .path(file_catalog::FileIconId::asset(self.name).path)
            .size_4()
    }
}
