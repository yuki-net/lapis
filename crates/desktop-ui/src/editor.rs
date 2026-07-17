use std::ops::Range;

use gpui::{
    App, Application, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point, Render, ShapedLine,
    SharedString, Style, TextRun, TitlebarOptions, UTF16Selection, Window, WindowBounds,
    WindowControlArea, WindowOptions, actions, div, fill, point, prelude::*, px, relative, rgb,
    rgba, size,
};
use lapis_app_services::{DocumentAction, EditorSession};

use crate::theme;

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
        ShowCommands,
        Dismiss,
        TogglePreview,
        ToggleBottomPanel,
        ToggleAssistant,
        Quit,
    ]
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolPanel {
    Files,
    Search,
    Git,
    History,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Preview,
    Assistant,
}

#[derive(Clone, Copy)]
enum ResizeTarget {
    ToolIsland,
    SidePanel,
    BottomPanel,
}

pub fn run(session: EditorSession) {
    Application::new().run(move |cx: &mut App| {
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Lapis".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let editor = cx.new(|cx| Editor::new(session, cx));
                window.focus(&editor.read(cx).focus_handle);
                editor
            },
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
        KeyBinding::new("ctrl-a", SelectAll, Some("Editor")),
        KeyBinding::new("cmd-v", Paste, Some("Editor")),
        KeyBinding::new("ctrl-v", Paste, Some("Editor")),
        KeyBinding::new("cmd-c", Copy, Some("Editor")),
        KeyBinding::new("ctrl-c", Copy, Some("Editor")),
        KeyBinding::new("cmd-x", Cut, Some("Editor")),
        KeyBinding::new("ctrl-x", Cut, Some("Editor")),
        KeyBinding::new("home", Home, Some("Editor")),
        KeyBinding::new("end", End, Some("Editor")),
        KeyBinding::new("enter", Enter, Some("Editor")),
        KeyBinding::new("cmd-o", Open, None),
        KeyBinding::new("ctrl-o", Open, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("ctrl-s", Save, None),
        KeyBinding::new("cmd-n", New, None),
        KeyBinding::new("ctrl-n", New, None),
        KeyBinding::new("cmd-shift-k", ShowCommands, None),
        KeyBinding::new("ctrl-shift-k", ShowCommands, None),
        KeyBinding::new("escape", Dismiss, None),
        KeyBinding::new("ctrl-alt-p", TogglePreview, None),
        KeyBinding::new("ctrl-j", ToggleBottomPanel, None),
        KeyBinding::new("ctrl-shift-a", ToggleAssistant, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-q", Quit, None),
    ]);
}

pub struct Editor {
    session: EditorSession,
    focus_handle: FocusHandle,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    status: String,
    active_tool: ToolPanel,
    command_palette_open: bool,
    side_panel: Option<SidePanel>,
    bottom_panel_open: bool,
    tool_island_width: f32,
    side_panel_width: f32,
    bottom_panel_height: f32,
    resizing: Option<ResizeTarget>,
}

impl Editor {
    fn new(session: EditorSession, cx: &mut Context<Self>) -> Self {
        Self {
            session,
            focus_handle: cx.focus_handle(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            status: "新しい Markdown ドキュメント".to_owned(),
            active_tool: ToolPanel::Files,
            command_palette_open: false,
            side_panel: None,
            bottom_panel_open: false,
            tool_island_width: theme::TOOL_ISLAND_WIDTH,
            side_panel_width: theme::SIDE_PANEL_WIDTH,
            bottom_panel_height: theme::BOTTOM_PANEL_HEIGHT,
            resizing: None,
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
        self.session
            .content()
            .char_indices()
            .map(|(index, _)| index)
            .rev()
            .find(|index| *index < offset)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.session
            .content()
            .char_indices()
            .map(|(index, _)| index)
            .find(|index| *index > offset)
            .unwrap_or(self.session.content().len())
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
        for ch in self.session.content().chars() {
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
        for ch in self.session.content().chars() {
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
        self.session.replace_range(range.clone(), text);
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
        self.selected_range = 0..self.session.content().len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let start = self.session.content()[..cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        self.move_to(start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let end = self.session.content()[cursor..]
            .find('\n')
            .map(|index| cursor + index)
            .unwrap_or(self.session.content().len());
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
                self.session.content()[self.selected_range.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                self.session.content()[self.selected_range.clone()].to_owned(),
            ));
        }
        self.replace_selection("", cx);
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.command_palette_open = false;
        self.open_file(window, cx);
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.command_palette_open = false;
        self.save_file(window, cx);
    }

    fn new_document(&mut self, _: &New, _: &mut Window, cx: &mut Context<Self>) {
        self.session.new_document();
        self.selected_range = 0..0;
        self.status = "新しい Markdown ドキュメント".to_owned();
        self.command_palette_open = false;
        cx.notify();
    }

    fn show_commands(&mut self, _: &ShowCommands, _: &mut Window, cx: &mut Context<Self>) {
        self.command_palette_open = !self.command_palette_open;
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette_open {
            self.command_palette_open = false;
            cx.notify();
        }
    }

    fn toggle_preview(&mut self, _: &TogglePreview, _: &mut Window, cx: &mut Context<Self>) {
        self.side_panel = if self.side_panel == Some(SidePanel::Preview) {
            None
        } else {
            Some(SidePanel::Preview)
        };
        self.command_palette_open = false;
        cx.notify();
    }

    fn toggle_bottom_panel(
        &mut self,
        _: &ToggleBottomPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bottom_panel_open = !self.bottom_panel_open;
        self.command_palette_open = false;
        cx.notify();
    }

    fn toggle_assistant(&mut self, _: &ToggleAssistant, _: &mut Window, cx: &mut Context<Self>) {
        self.side_panel = if self.side_panel == Some(SidePanel::Assistant) {
            None
        } else {
            Some(SidePanel::Assistant)
        };
        self.command_palette_open = false;
        cx.notify();
    }

    fn select_tool(&mut self, panel: ToolPanel, cx: &mut Context<Self>) {
        self.active_tool = panel;
        cx.notify();
    }

    fn start_resize(&mut self, target: ResizeTarget, cx: &mut Context<Self>) {
        self.resizing = Some(target);
        cx.notify();
    }

    fn resize_panels(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            return;
        }
        let Some(target) = self.resizing else {
            return;
        };

        let viewport = window.viewport_size();
        match target {
            ResizeTarget::ToolIsland => {
                self.tool_island_width =
                    (f32::from(event.position.x) - theme::CANVAS_GAP).clamp(190.0, 380.0);
            }
            ResizeTarget::SidePanel => {
                self.side_panel_width = (f32::from(viewport.width - event.position.x)
                    - theme::CANVAS_GAP)
                    .clamp(260.0, 480.0);
            }
            ResizeTarget::BottomPanel => {
                self.bottom_panel_height = (f32::from(viewport.height - event.position.y)
                    - theme::CANVAS_GAP)
                    .clamp(140.0, 360.0);
            }
        }
        cx.notify();
    }

    fn stop_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            cx.notify();
        }
    }

    fn focus_editor(&mut self, _: &MouseDownEvent, window: &mut Window, _: &mut Context<Self>) {
        window.focus(&self.focus_handle);
    }

    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.open_document() {
            Ok(DocumentAction::Completed) => {
                self.selected_range = 0..0;
                self.status = "読み込みました".to_owned();
                window.focus(&self.focus_handle);
                cx.notify();
            }
            Ok(DocumentAction::Cancelled) => {}
            Err(error) => self.status = format!("読み込み失敗: {error}"),
        }
    }

    fn save_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.save_document() {
            Ok(DocumentAction::Completed) => {
                self.status = "保存しました".to_owned();
                window.focus(&self.focus_handle);
                cx.notify();
            }
            Ok(DocumentAction::Cancelled) => {}
            Err(error) => self.status = format!("保存失敗: {error}"),
        }
    }

    fn render_command_palette(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .absolute()
            .top(px(49.0))
            .left(relative(0.5))
            .ml(px(-220.0))
            .w(px(440.0))
            .p(px(8.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(rgb(0x3d4050))
            .bg(theme::surface())
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .h(px(31.0))
                    .px_2()
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(theme::island())
                    .text_size(px(12.0))
                    .text_color(theme::muted())
                    .child("⌕")
                    .child("Commands"),
            )
            .child(
                command_item("新しい Markdown", "Ctrl N").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.new_document(&New, window, cx);
                    },
                )),
            )
            .child(
                command_item("Markdown を開く…", "Ctrl O").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.command_palette_open = false;
                        this.open_file(window, cx);
                    },
                )),
            )
            .child(
                command_item("保存", "Ctrl S").on_click(cx.listener(|this, _, window, cx| {
                    this.command_palette_open = false;
                    this.save_file(window, cx);
                })),
            )
            .child(
                command_item("Markdown Preview", "Ctrl Alt P").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.toggle_preview(&TogglePreview, window, cx);
                    },
                )),
            )
            .child(command_item("Bottom Panel", "Ctrl J").on_click(cx.listener(
                |this, _, window, cx| {
                    this.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
                },
            )))
            .child(
                command_item("AI Assistant", "Ctrl Shift A").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.toggle_assistant(&ToggleAssistant, window, cx);
                    },
                )),
            )
            .child(
                div()
                    .pt_1()
                    .px_2()
                    .text_size(px(10.0))
                    .text_color(theme::subtle())
                    .child("Esc で閉じる"),
            )
    }

    fn render_tool_content(&self) -> gpui::Div {
        match self.active_tool {
            ToolPanel::Files => div()
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
                        .child(self.session.display_name())
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_color(if self.session.is_dirty() {
                                    theme::accent()
                                } else {
                                    theme::subtle()
                                })
                                .child(if self.session.is_dirty() { "●" } else { "" }),
                        ),
                ),
            ToolPanel::Search => tool_empty_state(
                "⌕",
                "Search",
                "検索バックエンドは未接続です",
                "ワークスペース検索は境界実装後に利用できます",
            ),
            ToolPanel::Git => tool_empty_state(
                "⑂",
                "Git",
                "Git バックエンドは未接続です",
                "変更内容を推測で表示しません",
            ),
            ToolPanel::History => div()
                .flex()
                .flex_col()
                .flex_1()
                .p(px(10.0))
                .gap_2()
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme::subtle())
                        .child("DOCUMENT HISTORY"),
                )
                .child(
                    div()
                        .p_2()
                        .rounded(px(6.0))
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_size(px(12.0))
                        .child(
                            div()
                                .text_color(theme::text())
                                .child(format!("Revision {}", self.session.revision())),
                        )
                        .child(div().text_size(px(11.0)).text_color(theme::subtle()).child(
                            if self.session.is_dirty() {
                                "未保存の変更があります"
                            } else {
                                "保存済み"
                            },
                        )),
                ),
        }
    }

    fn preview_lines(&self) -> Vec<gpui::Div> {
        self.session
            .content()
            .lines()
            .map(|line| {
                let (text, size, color) = if let Some(value) = line.strip_prefix("# ") {
                    (value.to_owned(), px(24.0), theme::text())
                } else if let Some(value) = line.strip_prefix("## ") {
                    (value.to_owned(), px(19.0), theme::text())
                } else if let Some(value) = line.strip_prefix("### ") {
                    (value.to_owned(), px(16.0), theme::text())
                } else if let Some(value) = line.strip_prefix("- ") {
                    (format!("• {value}"), px(13.0), theme::muted())
                } else if let Some(value) = line.strip_prefix("> ") {
                    (format!("│ {value}"), px(13.0), rgb(0xb8b9f8))
                } else {
                    (line.to_owned(), px(13.0), theme::muted())
                };
                div()
                    .min_h(px(22.0))
                    .text_size(size)
                    .text_color(color)
                    .child(text)
            })
            .collect()
    }

    fn render_side_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let panel = self.side_panel.unwrap_or(SidePanel::Preview);
        let (icon, title) = match panel {
            SidePanel::Preview => ("◫", "Preview"),
            SidePanel::Assistant => ("✦", "AI Assistant"),
        };

        div()
            .w(px(self.side_panel_width))
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .border_1()
            .border_color(theme::border())
            .bg(theme::island())
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(39.0))
                    .flex_shrink_0()
                    .px_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(theme::text())
                    .child(
                        div()
                            .text_color(if panel == SidePanel::Assistant {
                                rgb(0xb8b9f8)
                            } else {
                                theme::muted()
                            })
                            .child(icon),
                    )
                    .child(title)
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("close-side-panel")
                            .size(px(25.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::muted())
                            .hover(|style| {
                                style.bg(theme::surface_hover()).text_color(theme::text())
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.side_panel = None;
                                cx.notify();
                            }))
                            .child("×"),
                    ),
            )
            .when_else(
                panel == SidePanel::Preview,
                |panel| {
                    panel.child(
                        div()
                            .id("preview-scroll")
                            .h(px(0.0))
                            .min_h(px(0.0))
                            .flex_1()
                            .overflow_y_scroll()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when_else(
                                self.session.content().is_empty(),
                                |preview| {
                                    preview.child(panel_empty_state(
                                        "◫",
                                        "プレビューする内容がありません",
                                        "Markdown を入力するとここに反映されます",
                                    ))
                                },
                                |preview| preview.children(self.preview_lines()),
                            ),
                    )
                },
                |panel| {
                    panel.child(
                        div()
                            .flex_1()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(34.0))
                                    .rounded(px(8.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(theme::accent_soft())
                                    .text_color(rgb(0xb8b9f8))
                                    .child("✦"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(theme::text())
                                    .child("AI Assistant"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme::muted())
                                    .child("Assistant バックエンドは未接続です。"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme::subtle())
                                    .child("接続後、この文書の内容を参照できるようになります。"),
                            ),
                    )
                },
            )
    }

    fn render_bottom_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .h(px(self.bottom_panel_height))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .border_1()
            .border_color(theme::border())
            .bg(theme::island())
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.0))
                    .flex_shrink_0()
                    .px_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(panel_tab("Terminal", true))
                    .child(panel_tab("Problems", false))
                    .child(panel_tab("Output", false))
                    .child(panel_tab("Debug", false))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("close-bottom-panel")
                            .size(px(25.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::muted())
                            .hover(|style| {
                                style.bg(theme::surface_hover()).text_color(theme::text())
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.bottom_panel_open = false;
                                cx.notify();
                            }))
                            .child("×"),
                    ),
            )
            .child(panel_empty_state(
                ">_",
                "Terminal バックエンドは未接続です",
                "外部 CLI の出力形式を UI に直接公開しません",
            ))
    }

    fn cursor_line_column(&self) -> (usize, usize) {
        let cursor = self.cursor_offset();
        let before_cursor = &self.session.content()[..cursor];
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty_marker = if self.session.is_dirty() { " *" } else { "" };
        let display_name = self.session.display_name();
        let document_is_empty = self.session.content().is_empty();
        let editor_focused = self.focus_handle.is_focused(window);
        let compact_layout = f32::from(window.viewport_size().width) < 1080.0;
        let status_is_error = self.status.contains("失敗");
        let maximize_label = if window.is_maximized() { "❐" } else { "□" };
        let (line, column) = self.cursor_line_column();

        div()
            .size_full()
            .relative()
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
            .on_action(cx.listener(Self::show_commands))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::toggle_bottom_panel))
            .on_action(cx.listener(Self::toggle_assistant))
            .on_mouse_move(cx.listener(Self::resize_panels))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.stop_resize(cx)),
            )
            .child(
                div()
                    .h(px(theme::TITLE_BAR_HEIGHT))
                    .w_full()
                    .flex_shrink_0()
                    .pl(px(12.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .bg(theme::title_bar())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .w(px(if compact_layout { 200.0 } else { 320.0 }))
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
                            .child(
                                top_icon(
                                    "▥",
                                    self.side_panel == Some(SidePanel::Preview),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_preview(&TogglePreview, window, cx);
                                })),
                            )
                            .child(
                                top_icon("▱", self.bottom_panel_open).on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .w(px(0.0))
                            .h(px(
                                theme::TITLE_BAR_HEIGHT - theme::WINDOW_RESIZE_BORDER_HEIGHT,
                            ))
                            .mt(px(theme::WINDOW_RESIZE_BORDER_HEIGHT))
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .text_size(px(12.0))
                            .window_control_area(WindowControlArea::Drag)
                            .child(div().text_color(theme::muted()).child("lapis"))
                            .child(div().text_color(theme::subtle()).child("›"))
                            .child(div().text_color(theme::subtle()).child("local")),
                    )
                    .child(
                        div()
                            .w(px(if compact_layout { 150.0 } else { 220.0 }))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_1()
                            .child(
                                div()
                                    .id("assistant-toggle")
                                    .h(px(27.0))
                                    .px_2()
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(11.0))
                                    .text_color(rgb(0x8da8ff))
                                    .bg(if self.side_panel == Some(SidePanel::Assistant) {
                                        theme::accent_soft()
                                    } else {
                                        theme::title_bar()
                                    })
                                    .hover(|style| style.bg(theme::surface_hover()))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_assistant(&ToggleAssistant, window, cx);
                                    }))
                                    .child("✦")
                                    .child("Note"),
                            )
                            .child(top_icon("⌕", false))
                            .child(top_icon("▷", false)),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_end()
                            .child(window_control_button(
                                "window-minimize",
                                "—",
                                WindowControlArea::Min,
                                false,
                            ))
                            .child(window_control_button(
                                "window-maximize",
                                maximize_label,
                                WindowControlArea::Max,
                                false,
                            ))
                            .child(window_control_button(
                                "window-close",
                                "×",
                                WindowControlArea::Close,
                                true,
                            )),
                    ),
            )
            .child(
                div()
                    .h(px(0.0))
                    .w_full()
                    .min_h(px(0.0))
                    .flex()
                    .flex_1()
                    .p(px(theme::CANVAS_GAP))
                    .child(
                        div()
                            .w(px(self.tool_island_width))
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
                                    .child(tool_tab(
                                        "Files",
                                        self.active_tool == ToolPanel::Files,
                                    ).on_click(cx.listener(|this, _, _, cx| {
                                        this.select_tool(ToolPanel::Files, cx);
                                    })))
                                    .child(tool_tab(
                                        "Search",
                                        self.active_tool == ToolPanel::Search,
                                    ).on_click(cx.listener(|this, _, _, cx| {
                                        this.select_tool(ToolPanel::Search, cx);
                                    })))
                                    .child(tool_tab(
                                        "Git",
                                        self.active_tool == ToolPanel::Git,
                                    ).on_click(cx.listener(|this, _, _, cx| {
                                        this.select_tool(ToolPanel::Git, cx);
                                    })))
                                    .child(tool_tab(
                                        "History",
                                        self.active_tool == ToolPanel::History,
                                    ).on_click(cx.listener(|this, _, _, cx| {
                                        this.select_tool(ToolPanel::History, cx);
                                    })))
                                    .child(div().flex_1())
                                    .child(top_icon("+", self.command_palette_open).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.command_palette_open = !this.command_palette_open;
                                            cx.notify();
                                        }),
                                    )),
                            )
                            .child(self.render_tool_content()),
                    )
                    .child(
                        div()
                            .w(px(theme::CANVAS_GAP))
                            .flex_shrink_0()
                            .cursor(CursorStyle::ResizeLeftRight)
                            .hover(|style| style.bg(theme::accent_soft()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.start_resize(ResizeTarget::ToolIsland, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .w(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .relative()
                                    .overflow_hidden()
                                    .rounded(px(theme::ISLAND_RADIUS))
                                    .border_1()
                                    .border_color(if editor_focused {
                                        rgb(0x3a3c58)
                                    } else {
                                        theme::border()
                                    })
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
                                            .child(
                                                div().text_color(rgb(0x8da8ff)).child("✓ Note"),
                                            )
                                            .child(format!("R{}", self.session.revision()))
                                            .child(format!("Ln {line}, Col {column}"))
                                            .child("·")
                                            .child(
                                                div()
                                                    .text_color(if status_is_error {
                                                        rgb(0xf18f96)
                                                    } else {
                                                        theme::subtle()
                                                    })
                                                    .child(self.status.clone()),
                                            ),
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
                                                    .relative()
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
                                                    })
                                                    .when(document_is_empty, |canvas| {
                                                        canvas.child(
                                                            div()
                                                                .absolute()
                                                                .top(px(72.0))
                                                                .left(px(54.0))
                                                                .w(px(330.0))
                                                                .flex()
                                                                .flex_col()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .text_size(px(18.0))
                                                                        .text_color(theme::text())
                                                                        .child("Markdown を始める"),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .mb_1()
                                                                        .text_size(px(12.0))
                                                                        .text_color(theme::subtle())
                                                                        .child("新規作成するか、既存の文書を開きます"),
                                                                )
                                                                .child(
                                                                    quick_action(
                                                                        "Markdown を開く…",
                                                                        "Ctrl O",
                                                                    )
                                                                    .on_click(cx.listener(
                                                                        |this, _, window, cx| {
                                                                            this.open_file(
                                                                                window, cx,
                                                                            );
                                                                        },
                                                                    )),
                                                                )
                                                                .child(
                                                                    quick_action(
                                                                        "新しい文書",
                                                                        "Ctrl N",
                                                                    )
                                                                    .on_click(cx.listener(
                                                                        |this, _, window, cx| {
                                                                            this.new_document(
                                                                                &New, window, cx,
                                                                            );
                                                                        },
                                                                    )),
                                                                )
                                                                .child(
                                                                    quick_action(
                                                                        "すべてのコマンド",
                                                                        "Ctrl Shift K",
                                                                    )
                                                                    .on_click(cx.listener(
                                                                        |this, _, _, cx| {
                                                                            this.command_palette_open = true;
                                                                            cx.notify();
                                                                        },
                                                                    )),
                                                                ),
                                                        )
                                                    }),
                                            ),
                                    ),
                            )
                            .when(self.bottom_panel_open, |center| {
                                center
                                    .child(
                                        div()
                                            .h(px(theme::CANVAS_GAP))
                                            .flex_shrink_0()
                                            .cursor(CursorStyle::ResizeUpDown)
                                            .hover(|style| style.bg(theme::accent_soft()))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.start_resize(
                                                        ResizeTarget::BottomPanel,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    )
                                    .child(self.render_bottom_panel(cx))
                            }),
                    )
                    .when(self.side_panel.is_some(), |body| {
                        body.child(
                            div()
                                .w(px(theme::CANVAS_GAP))
                                .flex_shrink_0()
                                .cursor(CursorStyle::ResizeLeftRight)
                                .hover(|style| style.bg(theme::accent_soft()))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.start_resize(ResizeTarget::SidePanel, cx);
                                    }),
                                ),
                        )
                            .child(self.render_side_panel(cx))
                    }),
            )
            .when(self.command_palette_open, |root| {
                root.child(self.render_command_palette(cx))
            })
    }
}

fn top_icon(label: &'static str, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(label)
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

fn window_control_button(
    id: &'static str,
    label: &'static str,
    area: WindowControlArea,
    close: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(
            theme::TITLE_BAR_HEIGHT - theme::WINDOW_RESIZE_BORDER_HEIGHT
        ))
        .w(px(theme::WINDOW_CONTROL_WIDTH))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .text_size(px(14.0))
        .text_color(theme::muted())
        .hover(move |style| {
            if close {
                style.bg(theme::close_hover()).text_color(rgb(0xffffff))
            } else {
                style.bg(theme::surface_hover()).text_color(theme::text())
            }
        })
        .active(|style| style.bg(theme::surface_active()))
        .child(label)
}

fn tool_tab(label: &'static str, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(label)
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

fn tool_empty_state(
    icon: &'static str,
    title: &'static str,
    message: &'static str,
    detail: &'static str,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .px_3()
        .gap_2()
        .text_center()
        .child(
            div()
                .size(px(34.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::surface())
                .text_size(px(17.0))
                .text_color(theme::muted())
                .child(icon),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme::text())
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme::muted())
                .child(message),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(detail),
        )
}

fn panel_empty_state(icon: &'static str, message: &'static str, detail: &'static str) -> gpui::Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .text_center()
        .child(
            div()
                .text_size(px(18.0))
                .text_color(theme::subtle())
                .child(icon),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme::muted())
                .child(message),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(detail),
        )
}

fn panel_tab(label: &'static str, active: bool) -> gpui::Div {
    div()
        .h(px(28.0))
        .px_2()
        .rounded(px(5.0))
        .flex()
        .items_center()
        .bg(if active {
            theme::surface()
        } else {
            theme::island()
        })
        .text_size(px(11.0))
        .text_color(if active {
            theme::text()
        } else {
            theme::muted()
        })
        .child(label)
}

fn command_item(label: &'static str, shortcut: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(label)
        .h(px(31.0))
        .px_2()
        .rounded(px(6.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .text_color(theme::text())
        .hover(|style| style.bg(theme::surface_active()))
        .child(label)
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(shortcut),
        )
}

fn quick_action(label: &'static str, shortcut: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(label)
        .h(px(34.0))
        .px_3()
        .rounded(px(7.0))
        .border_1()
        .border_color(theme::border())
        .bg(theme::surface())
        .flex()
        .items_center()
        .text_size(px(12.0))
        .text_color(theme::text())
        .hover(|style| style.bg(theme::surface_hover()).border_color(rgb(0x444657)))
        .child(label)
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(shortcut),
        )
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
        let line_count = self
            .editor
            .read(cx)
            .session
            .content()
            .lines()
            .count()
            .max(1);
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
        let content = editor.session.content();
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
        Some(self.session.content()[range].to_owned())
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
