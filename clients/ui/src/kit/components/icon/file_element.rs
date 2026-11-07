use gpui::{App, IntoElement, Pixels, RenderOnce, Styled, Window, svg};

use super::{FileIconId, file_catalog};

#[derive(IntoElement)]
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

impl RenderOnce for FileIcon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut icon = svg()
            .path(file_catalog::FileIconId::asset(self.name).path)
            .text_color(crate::theme::colors(cx).text_secondary);

        if let Some(size) = self.size {
            icon = icon.size(size);
        } else {
            icon = icon.size_4();
        }

        icon
    }
}
