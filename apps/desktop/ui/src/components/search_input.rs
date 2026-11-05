//! Popup内の検索に使う、IME対応の単一行入力。
use super::{Icon, IconName};
use crate::theme;
use gpui::{
    Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, Pixels, Point, Render, ShapedLine, UTF16Selection, Window, canvas,
    div, fill, point, prelude::*, px, size,
};
use std::ops::Range;

pub(crate) enum SearchInputEvent {
    Changed(String),
}

pub(crate) struct SearchInput {
    text: String,
    placeholder: String,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    focus: FocusHandle,
    line: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
}
impl gpui::EventEmitter<SearchInputEvent> for SearchInput {}
impl Focusable for SearchInput {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn byte_index(text: &str, utf16: usize) -> usize {
    let mut offset = 0;
    for (index, ch) in text.char_indices() {
        if offset + ch.len_utf16() > utf16 {
            return index;
        }
        offset += ch.len_utf16();
    }
    text.len()
}
fn utf16_index(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

impl SearchInput {
    pub(crate) fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self {
            text: String::new(),
            placeholder: placeholder.into(),
            selection: 0..0,
            marked: None,
            focus: cx.focus_handle(),
            line: None,
            bounds: None,
        }
    }
    pub(crate) fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.text = text;
        self.selection = self.text.len()..self.text.len();
        self.marked = None;
        cx.emit(SearchInputEvent::Changed(self.text.clone()));
        cx.notify();
    }
    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let text = text.replace(['\r', '\n'], " ");
        self.text.replace_range(range.clone(), &text);
        let end = range.start + text.len();
        self.selection = end..end;
        self.marked = None;
        cx.emit(SearchInputEvent::Changed(self.text.clone()));
        cx.notify();
    }
    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        let key = event.keystroke.key.as_str();
        match (command, key) {
            (true, "a") => self.selection = 0..self.text.len(),
            (true, "c") | (true, "x") => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                    self.text[self.selection.clone()].to_owned(),
                ));
                if key == "x" {
                    self.replace(self.selection.clone(), "", cx);
                }
            }
            (true, "v") => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.replace(self.selection.clone(), &text, cx);
                }
            }
            (false, "backspace") => {
                let range = if self.selection.is_empty() {
                    self.text[..self.selection.start]
                        .char_indices()
                        .last()
                        .map_or(0, |(index, _)| index)..self.selection.end
                } else {
                    self.selection.clone()
                };
                self.replace(range, "", cx);
            }
            (false, "delete") => {
                let range = if self.selection.is_empty() {
                    self.selection.start
                        ..self.selection.end
                            + self.text[self.selection.end..]
                                .chars()
                                .next()
                                .map_or(0, char::len_utf8)
                } else {
                    self.selection.clone()
                };
                self.replace(range, "", cx);
            }
            (false, "left") => {
                let cursor = if self.selection.is_empty() {
                    self.text[..self.selection.start]
                        .char_indices()
                        .last()
                        .map_or(0, |(index, _)| index)
                } else {
                    self.selection.start
                };
                self.selection = cursor..cursor;
            }
            (false, "right") => {
                let cursor = if self.selection.is_empty() {
                    self.selection.end
                        + self.text[self.selection.end..]
                            .chars()
                            .next()
                            .map_or(0, char::len_utf8)
                } else {
                    self.selection.end
                };
                self.selection = cursor..cursor;
            }
            (false, "home") => self.selection = 0..0,
            (false, "end") => self.selection = self.text.len()..self.text.len(),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl EntityInputHandler for SearchInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = byte_index(&self.text, range.start)..byte_index(&self.text, range.end);
        *actual = Some(utf16_index(&self.text, bytes.start)..utf16_index(&self.text, bytes.end));
        Some(self.text[bytes].to_owned())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: utf16_index(&self.text, self.selection.start)
                ..utf16_index(&self.text, self.selection.end),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| utf16_index(&self.text, range.start)..utf16_index(&self.text, range.end))
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bytes = range
            .map(|range| byte_index(&self.text, range.start)..byte_index(&self.text, range.end))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        self.replace(bytes, text, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bytes = range
            .map(|range| byte_index(&self.text, range.start)..byte_index(&self.text, range.end))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        let start = bytes.start;
        let text = text.replace(['\r', '\n'], " ");
        self.replace(bytes, &text, cx);
        self.marked = (!text.is_empty()).then_some(start..start + text.len());
        if let Some(range) = selection {
            self.selection =
                start + byte_index(&text, range.start)..start + byte_index(&text, range.end);
        }
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.line.as_ref()?;
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(byte_index(&self.text, range.start)),
                bounds.top(),
            ),
            point(
                bounds.left() + line.x_for_index(byte_index(&self.text, range.end)),
                bounds.bottom(),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(utf16_index(
            &self.text,
            self.line
                .as_ref()?
                .closest_index_for_x(position.x - self.bounds?.left())
                .min(self.text.len()),
        ))
    }
}

impl Render for SearchInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = cx.entity();
        let paint_input = input.clone();
        let focused = self.focus.is_focused(window);
        div()
            .id("search-input")
            .track_focus(&self.focus)
            .w_full()
            .h(px(32.))
            .px_2()
            .rounded(px(5.))
            .border_1()
            .border_color(if focused {
                theme::colors(cx).button_border_focused
            } else {
                theme::colors(cx).floating_border
            })
            .flex()
            .items_center()
            .gap_2()
            .overflow_hidden()
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    crate::gpui_compat::focus(window, &this.focus, cx);
                    let index = this.line.as_ref().zip(this.bounds).map_or(
                        this.text.len(),
                        |(line, bounds)| {
                            line.closest_index_for_x(event.position.x - bounds.left())
                                .min(this.text.len())
                        },
                    );
                    this.selection = index..index;
                    cx.notify();
                }),
            )
            .child(Icon::new(IconName::Search))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let input = input.read(cx);
                        let text: gpui::SharedString = if input.text.is_empty() {
                            input.placeholder.clone().into()
                        } else {
                            input.text.clone().into()
                        };
                        let mut style = window.text_style();
                        style.color = if input.text.is_empty() {
                            theme::colors(cx).text_tertiary.into()
                        } else {
                            theme::colors(cx).text_primary.into()
                        };
                        let line = window.text_system().shape_line(
                            text.clone(),
                            px(13.),
                            &[style.to_run(text.len())],
                            None,
                        );
                        (line, bounds)
                    },
                    move |_, (line, bounds), window, cx| {
                        let (focus, selection, empty) = paint_input.update(cx, |input, _| {
                            input.line = Some(line.clone());
                            input.bounds = Some(bounds);
                            (
                                input.focus.clone(),
                                input.selection.clone(),
                                input.text.is_empty(),
                            )
                        });
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint_input.clone()),
                            cx,
                        );
                        if focus.is_focused(window) && !selection.is_empty() {
                            window.paint_quad(fill(
                                Bounds::new(
                                    bounds.origin
                                        + point(line.x_for_index(selection.start), px(0.)),
                                    size(
                                        line.x_for_index(selection.end)
                                            - line.x_for_index(selection.start),
                                        px(22.),
                                    ),
                                ),
                                theme::colors(cx).editor_selection,
                            ));
                        }
                        crate::gpui_compat::paint_line(&line, bounds.origin, px(22.), window, cx);
                        if focus.is_focused(window) {
                            let x = if empty {
                                px(0.)
                            } else {
                                line.x_for_index(selection.end)
                            };
                            window.paint_quad(fill(
                                Bounds::new(
                                    bounds.origin + point(x, px(3.)),
                                    size(px(1.), px(16.)),
                                ),
                                theme::colors(cx).text_primary,
                            ));
                        }
                    },
                )
                .flex_1()
                .h(px(22.)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_indices_do_not_split_multibyte_search_text() {
        let text = "a枝🌿b";
        assert_eq!(byte_index(text, 2), 4);
        assert_eq!(byte_index(text, 3), 4);
        assert_eq!(byte_index(text, 4), 8);
        assert_eq!(utf16_index(text, 8), 4);
    }
}
