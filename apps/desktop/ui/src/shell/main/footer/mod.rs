use super::*;
use crate::components::{ButtonSize, button};

impl Editor {
    /// フッター（ステータスバー）を描画する。
    pub(super) fn render_footer(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let (line, column) = self.cursor_line_column();
        let mut path = div()
            .flex()
            .flex_1()
            .items_center()
            .gap(tokens::spacing::XS)
            .overflow_hidden();
        for (index, part) in self.footer_path_parts().iter().enumerate() {
            if index > 0 {
                path = path.child(div().text_color(theme::colors().muted).child("/"));
            }
            path = path.child(
                button(("footer-path", index), part.clone(), ButtonSize::Xs)
                    .text_size(tokens::typography::FONT_SM),
            );
        }

        let right = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(tokens::spacing::XS)
            .text_size(tokens::typography::FONT_SM)
            .child(self.footer_button("footer-position", format!("{line}:{column}")))
            .child(self.footer_button("footer-line-ending", self.footer_line_ending()))
            .child(self.footer_button("footer-encoding", self.footer_encoding()));

        div()
            .h(px(31.0))
            .w_full()
            .flex_shrink_0()
            .px(tokens::spacing::GAP)
            .flex()
            .items_center()
            .justify_between()
            .child(path)
            .child(right)
    }

    fn footer_path_parts(&self) -> Vec<String> {
        let Some(path) = self.session.active_path() else {
            return vec!["Untitled".to_owned()];
        };
        let relative = self
            .session
            .workspace_root()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path);
        let parts: Vec<_> = relative
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if parts.is_empty() {
            vec![self.session.display_name()]
        } else {
            parts
        }
    }

    fn footer_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
    ) -> gpui::Stateful<gpui::Div> {
        button(id, label.into(), ButtonSize::Xs).text_size(tokens::typography::FONT_SM)
    }

    fn footer_line_ending(&self) -> &'static str {
        self.session
            .active_text()
            .ok()
            .filter(|text| {
                text.contains(
                    "
",
                )
            })
            .map(|_| "CRLF")
            .unwrap_or("LF")
    }

    fn footer_encoding(&self) -> &'static str {
        match self.session.encoding() {
            lapis_app_services::Encoding::Utf8 | lapis_app_services::Encoding::Utf8Bom => "UTF-8",
            lapis_app_services::Encoding::Utf16Le => "UTF-16 LE",
            lapis_app_services::Encoding::Utf16Be => "UTF-16 BE",
        }
    }
}
