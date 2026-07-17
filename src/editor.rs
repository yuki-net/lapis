use std::ops::Range;

use gpui::{
    App, Application, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, Render, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, Window, WindowBounds, WindowOptions, actions, div, fill, point,
    prelude::*, px, relative, rgb, rgba, size,
};

use crate::{
    document::{Document, WorkspaceBackend},
    theme,
};

actions!(
    editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Enter,
        Paste,
        Cut,
        Copy,
        Open,
        Save,
        New,
        Quit,
    ]
);

pub fn run() {
    Application::new().run(|cx: &mut App| {
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(Editor::new),
        )
        .expect("Lapis window should open");
        cx.on_action(|_: &Quit, cx| cx.quit());
    });
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Editor")),
        KeyBinding::new("delete", Delete, Some("Editor")),
        KeyBinding::new("left", Left, Some("Editor")),
        KeyBinding::new("right", Right, Some("Editor")),
        KeyBinding::new("shift-left", SelectLeft, Some("Editor")),
        KeyBinding::new("shift-right", SelectRight, Some("Editor")),
        KeyBinding::new("cmd-a", SelectAll, Some("Editor")),
        KeyBinding::new("cmd-v", Paste, Some("Editor")),
        KeyBinding::new("cmd-c", Copy, Some("Editor")),
        KeyBinding::new("cmd-x", Cut, Some("Editor")),
        KeyBinding::new("home", Home, Some("Editor")),
        KeyBinding::new("end", End, Some("Editor")),
        KeyBinding::new("enter", Enter, Some("Editor")),
        KeyBinding::new("cmd-o", Open, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-n", New, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

pub struct Editor {
    document: Document,
    focus_handle: FocusHandle,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    status: String,
}

impl Editor {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            document: Document::new(),
            focus_handle: cx.focus_handle(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            status: "新しい Markdown ドキュメント".to_owned(),
        }
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.document
            .content
            .char_indices()
            .map(|(index, _)| index)
            .rev()
            .find(|index| *index < offset)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.document
            .content
            .char_indices()
            .map(|(index, _)| index)
            .find(|index| *index > offset)
            .unwrap_or(self.document.content.len())
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.document.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.document.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn replace_selection(&mut self, text: &str, cx: &mut Context<Self>) {
        let range = self.selected_range.clone();
        self.document.set_content(format!(
            "{}{}{}",
            &self.document.content[..range.start],
            text,
            &self.document.content[range.end..]
        ));
        let cursor = range.start + text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = self.previous_boundary(cursor)..cursor;
        }
        self.replace_selection("", cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = cursor..self.next_boundary(cursor);
        }
        self.replace_selection("", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.document.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let start = self.document.content[..cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        self.move_to(start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let end = self.document.content[cursor..]
            .find('\n')
            .map(|index| cursor + index)
            .unwrap_or(self.document.content.len());
        self.move_to(end, cx);
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.replace_selection("\n", cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                self.document.content[self.selected_range.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                self.document.content[self.selected_range.clone()].to_owned(),
            ));
        }
        self.replace_selection("", cx);
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(window, cx);
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.save_file(window, cx);
    }

    fn new_document(&mut self, _: &New, _: &mut Window, cx: &mut Context<Self>) {
        self.document = Document::new();
        self.selected_range = 0..0;
        self.status = "新しい Markdown ドキュメント".to_owned();
        cx.notify();
    }

    fn focus_editor(&mut self, _: &MouseDownEvent, window: &mut Window, _: &mut Context<Self>) {
        window.focus(&self.focus_handle);
    }

    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown", "mdown"])
            .pick_file()
        else {
            return;
        };
        match WorkspaceBackend::read_markdown(&path) {
            Ok(content) => {
                self.document = Document::from_file(path, content);
                self.selected_range = 0..0;
                self.status = "読み込みました".to_owned();
                window.focus(&self.focus_handle);
                cx.notify();
            }
            Err(error) => self.status = format!("読み込み失敗: {error}"),
        }
    }

    fn save_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.document.path.clone().or_else(|| {
            rfd::FileDialog::new()
                .add_filter("Markdown", &["md", "markdown", "mdown"])
                .set_file_name("Untitled.md")
                .save_file()
        });
        let Some(path) = path else { return };
        match WorkspaceBackend::write_markdown(&path, &self.document.content) {
            Ok(()) => {
                self.document.mark_saved(path);
                self.status = "保存しました".to_owned();
                window.focus(&self.focus_handle);
                cx.notify();
            }
            Err(error) => self.status = format!("保存失敗: {error}"),
        }
    }

    fn cursor_line_column(&self) -> (usize, usize) {
        let cursor = self.cursor_offset();
        let before_cursor = &self.document.content[..cursor];
        let line = before_cursor.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = before_cursor
            .rsplit('\n')
            .next()
            .map(|value| value.chars().count() + 1)
            .unwrap_or(1);
        (line, column)
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty_marker = if self.document.is_dirty() { " *" } else { "" };
        let display_name = self.document.display_name();
        let (line, column) = self.cursor_line_column();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .text_color(theme::text())
            .track_focus(&self.focus_handle(cx))
            .key_context("Editor")
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::new_document))
            .child(
                div()
                    .h(px(theme::TITLE_BAR_HEIGHT))
                    .w_full()
                    .flex_shrink_0()
                    .px(px(12.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .bg(theme::title_bar())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .w(px(320.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .size(px(22.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(theme::accent())
                                    .text_color(rgb(0xffffff))
                                    .text_size(px(11.0))
                                    .child("L"),
                            )
                            .child(top_icon("☰", false))
                            .child(top_icon("▤", true))
                            .child(top_icon("▥", false))
                            .child(top_icon("▱", false)),
                    )
                    .child(
                        div()
                            .w(px(480.0))
                            .flex_shrink()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .child(div().text_color(theme::muted()).child("lapis"))
                            .child(div().text_color(theme::subtle()).child("›"))
                            .child(div().text_color(theme::subtle()).child("local")),
                    )
                    .child(
                        div()
                            .w(px(320.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_1()
                            .child(
                                div()
                                    .h(px(27.0))
                                    .px_2()
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(11.0))
                                    .text_color(rgb(0x8da8ff))
                                    .child("✦")
                                    .child("Note"),
                            )
                            .child(top_icon("⌕", false))
                            .child(top_icon("▷", false)),
                    ),
            )
            .child(
                div()
                    .h(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_1()
                    .p(px(theme::CANVAS_GAP))
                    .child(
                        div()
                            .w(px(theme::TOOL_ISLAND_WIDTH))
                            .flex()
                            .flex_col()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .rounded(px(theme::ISLAND_RADIUS))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::island())
                            .child(
                                div()
                                    .h(px(39.0))
                                    .flex_shrink_0()
                                    .px(px(7.0))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .child(tool_tab("Files", true))
                                    .child(tool_tab("Search", false))
                                    .child(tool_tab("Git", false))
                                    .child(tool_tab("History", false))
                                    .child(div().flex_1())
                                    .child(top_icon("+", false)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .p(px(6.0))
                                    .child(
                                        div()
                                            .h(px(28.0))
                                            .px_2()
                                            .rounded(px(5.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .bg(theme::surface_active())
                                            .text_size(px(12.0))
                                            .text_color(theme::text())
                                            .child("⌄")
                                            .child("lapis"),
                                    )
                                    .child(
                                        div()
                                            .h(px(28.0))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .text_size(px(10.0))
                                            .text_color(theme::subtle())
                                            .child("OPEN DOCUMENTS"),
                                    )
                                    .child(
                                        div()
                                            .h(px(28.0))
                                            .px_2()
                                            .rounded(px(5.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .bg(theme::surface())
                                            .text_size(px(12.0))
                                            .text_color(theme::text())
                                            .child(file_badge("M", theme::orange()))
                                            .child(display_name.clone())
                                            .child(div().flex_1())
                                            .child(
                                                div()
                                                    .text_color(if self.document.is_dirty() {
                                                        theme::accent()
                                                    } else {
                                                        theme::subtle()
                                                    })
                                                    .child(if self.document.is_dirty() {
                                                        "●"
                                                    } else {
                                                        ""
                                                    }),
                                            ),
                                    ),
                            ),
                    )
                    .child(div().w(px(theme::CANVAS_GAP)).flex_shrink_0())
                    .child(
                        div()
                            .w(px(0.0))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .relative()
                            .overflow_hidden()
                            .rounded(px(theme::ISLAND_RADIUS))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::island())
                            .child(
                                div()
                                    .h(px(39.0))
                                    .flex_shrink_0()
                                    .px(px(7.0))
                                    .flex()
                                    .items_end()
                                    .gap_1()
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .child(
                                        div()
                                            .h(px(31.0))
                                            .w(px(180.0))
                                            .flex_shrink_0()
                                            .px_2()
                                            .rounded_t(px(6.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .bg(theme::surface())
                                            .text_size(px(12.0))
                                            .text_color(theme::text())
                                            .child(file_badge("M", theme::orange()))
                                            .child(display_name.clone())
                                            .child(
                                                div()
                                                    .text_color(theme::subtle())
                                                    .child(dirty_marker),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(31.0))
                                    .flex_shrink_0()
                                    .px(px(14.0))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .text_size(px(12.0))
                                    .text_color(theme::subtle())
                                    .child("lapis")
                                    .child("›")
                                    .child(
                                        div()
                                            .text_color(theme::muted())
                                            .child(display_name.clone()),
                                    )
                                    .child("·")
                                    .child(div().text_color(rgb(0x8da8ff)).child("✓ Note"))
                                    .child(format!("R{}", self.document.revision.number))
                                    .child(format!("Ln {line}, Col {column}"))
                                    .child("·")
                                    .child(self.status.clone()),
                            )
                            .child(
                                div()
                                    .h(px(0.0))
                                    .min_h(px(0.0))
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .id("source-scroll")
                                            .h(px(0.0))
                                            .min_h(px(0.0))
                                            .flex_1()
                                            .overflow_y_scroll()
                                            .px(px(18.0))
                                            .py(px(10.0))
                                            .cursor(CursorStyle::IBeam)
                                            .text_size(px(14.0))
                                            .text_color(theme::text())
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(Self::focus_editor),
                                            )
                                            .child(EditorElement {
                                                editor: cx.entity(),
                                            }),
                                    ),
                            ),
                    ),
            )
    }
}

fn top_icon(label: &'static str, active: bool) -> gpui::Div {
    div()
        .size(px(28.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(if active {
            theme::accent_soft()
        } else {
            theme::title_bar()
        })
        .text_color(if active {
            rgb(0xbfc0ff)
        } else {
            theme::muted()
        })
        .text_size(px(14.0))
        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
        .child(label)
}

fn tool_tab(label: &'static str, active: bool) -> gpui::Div {
    div()
        .h(px(31.0))
        .px(px(8.0))
        .rounded_t(px(6.0))
        .flex()
        .items_center()
        .bg(if active {
            theme::surface()
        } else {
            theme::island()
        })
        .text_color(if active {
            theme::text()
        } else {
            theme::muted()
        })
        .text_size(px(12.0))
        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
        .child(label)
}

fn file_badge(label: &'static str, color: gpui::Rgba) -> gpui::Div {
    div()
        .size(px(14.0))
        .rounded(px(3.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(color)
        .text_color(theme::canvas())
        .text_size(px(8.0))
        .child(label)
}

struct EditorElement {
    editor: Entity<Editor>,
}

struct EditorPrepaint {
    lines: Vec<(ShapedLine, Point<Pixels>)>,
    cursor: PaintQuad,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = EditorPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let line_count = self.editor.read(cx).document.content.lines().count().max(1);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(24.0 * line_count as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let editor = self.editor.read(cx);
        let content = &editor.document.content;
        let text_style = window.text_style();
        let mut lines = Vec::new();
        let line_height = px(24.0);
        let mut cursor_line = 0;
        let mut cursor_column = 0;
        let mut offset = 0;

        for (line_index, text) in content.split('\n').enumerate() {
            let run = TextRun {
                len: text.len(),
                font: text_style.font(),
                color: text_style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(text.to_owned()),
                px(16.0),
                &[run],
                None,
            );
            let origin = point(bounds.left(), bounds.top() + line_height * line_index);
            let cursor = editor.cursor_offset();
            if cursor >= offset && cursor <= offset + text.len() {
                cursor_line = line_index;
                cursor_column = cursor - offset;
            }
            lines.push((shaped, origin));
            offset += text.len() + 1;
        }

        let line = &lines[cursor_line].0;
        let cursor_x = line.x_for_index(cursor_column);
        let cursor_y = bounds.top() + line_height * cursor_line;
        let cursor = fill(
            Bounds::new(
                point(bounds.left() + cursor_x, cursor_y),
                size(px(2.0), line_height),
            ),
            rgba(0x7dd3fcff),
        );
        EditorPrepaint { lines, cursor }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        for (line, origin) in prepaint.lines.drain(..) {
            line.paint(origin, px(24.0), window, cx).ok();
        }
        if focus_handle.is_focused(window) {
            window.paint_quad(prepaint.cursor.clone());
        }
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.document.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(range) = range_utf16 {
            self.selected_range = self.range_from_utf16(&range);
        }
        self.replace_selection(new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(range) = range_utf16 {
            self.selected_range = self.range_from_utf16(&range);
        }
        self.replace_selection(new_text, cx);
        self.marked_range = Some(self.selected_range.clone());
        if let Some(range) = new_selected_range_utf16 {
            let cursor = self.selected_range.start + self.offset_from_utf16(range.end);
            self.move_to(cursor, cx);
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.cursor_offset()))
    }
}
