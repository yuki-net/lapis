use super::*;
use crate::{
    extension_ui::PanelPosition,
    shell::{DraggedPanelTab, PanelHost, PanelTab, ResizeTarget},
};

struct PanelTabDragPreview {
    label: String,
}

impl Render for PanelTabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(5.0))
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border())
            .text_color(theme::text())
            .text_size(px(11.0))
            .child(self.label.clone())
    }
}

impl Editor {
    /// 四つのPanelを同じレイアウト規則で描画する。
    pub(super) fn render_main(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        _compact_layout: bool,
    ) -> impl IntoElement {
        let editor_focused = self.focus_handle.is_focused(window);
        let viewport_width = f32::from(window.viewport_size().width);
        let left_width = if self.shell.left_panel.open {
            self.shell.left_panel.size + theme::CANVAS_GAP
        } else {
            0.0
        };
        let right_width = if self.shell.right_panel.open {
            self.shell.right_panel.size + theme::CANVAS_GAP
        } else {
            0.0
        };
        let center_width =
            (viewport_width - theme::CANVAS_GAP * 2.0 - left_width - right_width).max(320.0);

        div()
            .h(px(0.0))
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_1()
            .px(px(theme::CANVAS_GAP))
            .when(self.shell.left_panel.open, |body| {
                body.child(self.render_panel_window_frame(&self.shell.left_panel, cx))
                    .child(self.render_resize_handle(ResizeTarget::Left, false, cx))
            })
            .child(
                div()
                    .w(px(center_width))
                    .h_full()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(self.render_panel_window(&self.shell.main_panel, cx, editor_focused))
                    .when(self.shell.bottom_panel.open, |center| {
                        center
                            .child(self.render_resize_handle(ResizeTarget::Bottom, true, cx))
                            .child(self.render_panel_window_frame(&self.shell.bottom_panel, cx))
                    }),
            )
            .when(self.shell.right_panel.open, |body| {
                body.child(self.render_resize_handle(ResizeTarget::Right, false, cx))
                    .child(self.render_panel_window_frame(&self.shell.right_panel, cx))
            })
    }

    fn render_panel_window(
        &self,
        panel: &PanelHost,
        cx: &mut Context<Self>,
        editor_focused: bool,
    ) -> gpui::Div {
        self.render_panel_window_frame(panel, cx)
            .border_color(if editor_focused {
                theme::focus_border()
            } else {
                theme::border()
            })
    }

    fn render_resize_handle(
        &self,
        target: ResizeTarget,
        horizontal: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut handle = div()
            .flex_shrink_0()
            .hover(|style| style.bg(theme::accent_soft()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.start_resize(target, cx)),
            );
        if horizontal {
            handle = handle
                .h(px(theme::CANVAS_GAP))
                .cursor(CursorStyle::ResizeUpDown);
        } else {
            handle = handle
                .w(px(theme::CANVAS_GAP))
                .cursor(CursorStyle::ResizeLeftRight);
        }
        handle
    }

    fn render_panel_window_frame(&self, panel: &PanelHost, cx: &mut Context<Self>) -> gpui::Div {
        let position = panel.position;
        let is_bottom = panel.position == PanelPosition::Bottom;
        let size = if panel.position == PanelPosition::Main {
            div().w_full().flex_1().min_h(px(0.0))
        } else if is_bottom {
            div().h(px(panel.size)).w_full()
        } else {
            div().w(px(panel.size)).h_full()
        };
        size.flex_shrink_0()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .border_1()
            .border_color(theme::border())
            .bg(theme::island())
            .flex()
            .flex_col()
            .on_drop(cx.listener(move |this, drag: &DraggedPanelTab, _, cx| {
                this.move_panel_tab(drag.source_panel, position, drag.tab.clone(), cx);
            }))
            .child(self.render_tool_panel_header(panel, cx))
            .child(match panel.active.as_ref() {
                Some(PanelTab::Tool(view)) => self.render_tool_content(view, cx).into_any_element(),
                Some(PanelTab::Document(document_id)) => self
                    .render_document_content(document_id, cx)
                    .into_any_element(),
                None => self
                    .render_empty_panel(panel.position, cx)
                    .into_any_element(),
            })
    }

    fn render_tool_panel_header(&self, panel: &PanelHost, cx: &mut Context<Self>) -> gpui::Div {
        let tabs = panel.tabs.iter().enumerate().map(|(index, tab)| {
            let label = match tab {
                PanelTab::Tool(view) => self
                    .feature_registry
                    .panel_contributions(panel.position)
                    .into_iter()
                    .find(|contribution| contribution.view.as_ref() == Some(view))
                    .map(|contribution| self.locale.resolve(&contribution.title))
                    .unwrap_or_else(|| view.as_str().to_owned()),
                PanelTab::Document(document_id) => self
                    .session
                    .tabs()
                    .into_iter()
                    .find(|document| document.id == *document_id)
                    .map(|document| document.display_name)
                    .unwrap_or_else(|| "Document".to_owned()),
            };
            let active = panel.active.as_ref() == Some(tab);
            let tab = tab.clone();
            let position = panel.position;
            let drag = DraggedPanelTab {
                source_panel: position,
                tab: tab.clone(),
            };
            div()
                .id(("panel-tab", panel_key(position) * 100 + index as u32))
                .h(px(30.0))
                .px_2()
                .rounded_t(px(6.0))
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
                .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                .cursor(CursorStyle::OpenHand)
                .on_drag(drag, |drag: &DraggedPanelTab, _, _, cx| {
                    cx.new(|_| PanelTabDragPreview {
                        label: format!("{:?}", drag.tab),
                    })
                })
                .on_click(cx.listener(move |this, _, window, cx| match tab.clone() {
                    PanelTab::Tool(view) => this.select_view(position, view, cx),
                    PanelTab::Document(document_id) => {
                        this.select_panel_tab(
                            position,
                            PanelTab::Document(document_id.clone()),
                            cx,
                        );
                        this.persist_active_view();
                        if this.session.activate_document(&document_id) {
                            this.restore_active_view();
                            window.focus(&this.focus_handle);
                            cx.notify();
                        }
                    }
                }))
                .child(label)
        });
        let position = panel.position;
        div()
            .h(px(39.0))
            .flex_shrink_0()
            .px_2()
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap_1()
            .children(tabs)
            .child(div().flex_1())
            .when(
                panel
                    .active_tool()
                    .is_some_and(|view| view.as_str() == id::VIEW_TERMINAL),
                |bar| {
                    bar.child(
                        task_action_button("Start", false)
                            .on_click(cx.listener(|this, _, _, cx| this.start_terminal(cx))),
                    )
                    .child(
                        task_action_button("Send clipboard", false).on_click(
                            cx.listener(|this, _, _, cx| this.send_terminal_clipboard(cx)),
                        ),
                    )
                },
            )
            .child(
                div()
                    .id(("close-panel", panel_key(position)))
                    .size(px(25.0))
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme::muted())
                    .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if position != PanelPosition::Main {
                            this.shell.panel_mut(position).close();
                        }
                        this.refresh_feature_activation();
                        cx.notify();
                    }))
                    .child("×"),
            )
            .child(
                div()
                    .id(("open-tool", panel_key(position)))
                    .size(px(25.0))
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme::muted())
                    .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_tool_picker(position, cx);
                    }))
                    .child("+"),
            )
    }

    fn render_empty_panel(&self, position: PanelPosition, cx: &mut Context<Self>) -> gpui::Div {
        let title = match position {
            PanelPosition::Left => "Left Panel",
            PanelPosition::Bottom => "Bottom Panel",
            PanelPosition::Right => "Right Panel",
            PanelPosition::Main => "Main Panel",
        };
        panel_empty_state(
            "▤",
            title,
            "Open new tool or drag-n-drop tool from other panels",
        )
        .child(
            div()
                .id(("open-tool", panel_key(position)))
                .mt_2()
                .px_2()
                .py_1()
                .rounded(px(5.0))
                .border_1()
                .border_color(theme::border())
                .text_size(px(11.0))
                .hover(|style| style.bg(theme::surface_hover()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_tool_picker(position, cx);
                }))
                .child("Open Tool"),
        )
    }

    fn render_document_content(
        &self,
        document_id: &lapis_editor_core::DocumentId,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let active = self.session.active_document_id() == Some(document_id);
        if active {
            div()
                .h(px(0.0))
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .flex_1()
                .child(
                    div()
                        .id("source-scroll")
                        .h(px(0.0))
                        .min_h(px(0.0))
                        .flex_1()
                        .overflow_scroll()
                        .track_scroll(&self.editor_scroll)
                        .relative()
                        .px(px(18.0))
                        .py(px(10.0))
                        .cursor(CursorStyle::IBeam)
                        .text_size(px(14.0))
                        .text_color(theme::text())
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::editor_mouse_down))
                        .on_mouse_move(cx.listener(Self::editor_mouse_move))
                        .on_mouse_up(MouseButton::Left, cx.listener(Self::editor_mouse_up))
                        .child(EditorElement { editor: cx.entity() })
                        .when(self.session.is_empty(), |canvas| {
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
                                            .child("Open a file or project"),
                                    )
                                    .child(
                                        div()
                                            .mb_1()
                                            .text_size(px(12.0))
                                            .text_color(theme::subtle())
                                            .child("Choose a project to restore its last workspace, or open a file to start here."),
                                    )
                                    .child(quick_action("Open file", "Ctrl O").on_click(
                                        cx.listener(|this, _, window, cx| this.open_file(window, cx)),
                                    ))
                                    .child(quick_action("Open project", "").on_click(
                                        cx.listener(|this, _, window, cx| this.open_project(window, cx)),
                                    ))
                                    .child(quick_action("New file", "Ctrl N").on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.new_document(&New, window, cx)
                                        }),
                                    )),
                            )
                        }),
                )
        } else {
            panel_empty_state("F", "Document", "Select the document tab to edit")
        }
    }

    fn render_tool_content(&self, view: &ViewId, cx: &mut Context<Self>) -> gpui::Div {
        match view.as_str() {
            id::VIEW_FILES => self.render_files_content(cx),
            id::VIEW_SEARCH => self.render_search_content(cx),
            id::VIEW_GIT => self.render_git_content(cx),
            id::VIEW_HISTORY => self.render_history_content(),
            id::VIEW_PREVIEW => self.render_preview_content(),
            id::VIEW_ASSISTANT => self.render_assistant_content(cx),
            id::VIEW_TERMINAL => self.render_terminal_content(),
            id::VIEW_PROBLEMS => self.render_problems_content(),
            id::VIEW_OUTPUT => self.render_output_content(),
            id::VIEW_COMMAND_SEARCH => div().flex_1().child(self.quick_search.clone()),
            id::VIEW_SETTINGS => self.render_settings_content(),
            _ => panel_empty_state("?", "Unknown view", view.as_str().to_owned()),
        }
    }

    fn render_settings_content(&self) -> gpui::Div {
        div()
            .flex_1()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_center()
            .child(Icon::new(IconName::Settings))
            .child(
                div()
                    .text_size(px(18.0))
                    .text_color(theme::text())
                    .child("Settings"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::muted())
                    .child("Settings will be available here."),
            )
    }

    pub(super) fn render_settings_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let anchor = self.shell.settings_menu_anchor;
        anchored()
            .position(anchor)
            .offset(point(px(-250.0), px(8.0)))
            .snap_to_window_with_margin(px(8.0))
            .child(
                div()
                    .id("settings-menu")
                    .w(px(250.0))
                    .p_2()
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .shadow_lg()
                    .text_color(theme::text())
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.close_settings_menu(cx);
                    }))
                    .child(
                        settings_menu_item(
                            IconName::Settings,
                            "Settings",
                            Some("Ctrl+,".to_owned()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_settings_view(cx);
                        })),
                    )
                    .child(
                        settings_menu_item(
                            IconName::SunMoon,
                            "Theme",
                            theme::name(&theme::active_id()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_theme_preference(cx);
                        })),
                    )
                    .when(self.shell.theme_picker_open, |menu| {
                        menu.child(self.render_theme_picker(cx))
                    }),
            )
    }

    fn render_theme_picker(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let active = theme::active_id();
        div()
            .id("theme-picker")
            .mt_1()
            .mb_1()
            .pl_2()
            .flex()
            .flex_col()
            .gap_1()
            .children(theme::available().into_iter().map(|(theme_id, name)| {
                let selected = active == theme_id;
                let click_id = theme_id.clone();
                div()
                    .id(gpui::SharedString::from(theme_id.as_str().to_owned()))
                    .h(px(30.0))
                    .w_full()
                    .px_2()
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(theme::text())
                    .hover(|style| style.bg(theme::surface_hover()))
                    .when(selected, |item| item.bg(theme::accent_soft()))
                    .child(if selected { "✓" } else { "" })
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_theme(click_id.clone(), cx);
                    }))
            }))
    }

    pub(super) fn render_tool_picker(
        &self,
        position: PanelPosition,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = self.shell.tool_picker_query.trim().to_lowercase();
        let tools = self
            .feature_registry
            .tool_contributions(position)
            .into_iter()
            .filter(|contribution| {
                if query.is_empty() {
                    return true;
                }
                let title = self.locale.resolve(&contribution.title).to_lowercase();
                let view = contribution
                    .view
                    .as_ref()
                    .map(|view| view.as_str().to_lowercase())
                    .unwrap_or_default();
                title.contains(&query) || view.contains(&query)
            })
            .filter_map(|contribution| {
                Some((
                    contribution.view.clone()?,
                    self.locale.resolve(&contribution.title),
                    contribution.icon.as_str().to_owned(),
                ))
            })
            .collect::<Vec<_>>();

        anchored()
            .position(point(px(120.0), px(82.0)))
            .snap_to_window_with_margin(px(8.0))
            .child(
                div()
                    .id("tool-picker")
                    .w(px(250.0))
                    .max_h(px(520.0))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .shadow_lg()
                    .text_color(theme::text())
                    .child(div().px_2().py_1().text_color(theme::muted()).child(
                        if self.shell.tool_picker_query.is_empty() {
                            "Search".to_owned()
                        } else {
                            self.shell.tool_picker_query.clone()
                        },
                    ))
                    .child(div().h(px(1.0)).my_1().bg(theme::border()))
                    .children(tools.into_iter().map(|(view, title, icon)| {
                        div()
                            .id(ElementId::Name(
                                format!("tool-picker-{}", view.as_str()).into(),
                            ))
                            .w_full()
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .hover(|style| style.bg(theme::surface_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_tool_from_picker(position, view.clone(), cx);
                            }))
                            .child(icon)
                            .child(title)
                    })),
            )
    }

    fn render_history_content(&self) -> gpui::Div {
        div()
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
                    .text_size(px(12.0))
                    .child(format!("Revision {}", self.session.revision())),
            )
    }

    fn render_preview_content(&self) -> gpui::Div {
        div()
            .h(px(0.0))
            .min_h(px(0.0))
            .flex_1()
            .overflow_hidden()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(if self.session.is_empty() {
                panel_empty_state(
                    "▱",
                    "No preview available",
                    "Open a Markdown document to preview it",
                )
            } else {
                div().children(self.preview_lines())
            })
    }

    #[allow(dead_code)]
    fn render_center_panel(
        &self,
        cx: &mut Context<Self>,
        editor_focused: bool,
        document_tabs: Vec<lapis_app_services::DocumentTab>,
    ) -> impl IntoElement {
        div()
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .border_1()
            .border_color(if editor_focused {
                theme::focus_border()
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
                    .id("editor-tabs-scroll")
                    .overflow_x_scroll()
                    .border_b_1()
                    .border_color(theme::border())
                    .children(document_tabs.into_iter().enumerate().map(|(tab_index, tab)| {
                        let tab_id = tab.id.clone();
                        div()
                            .id(("editor-tab", tab_index))
                            .h(px(31.0))
                            .w(px(180.0))
                            .flex_shrink_0()
                            .px_2()
                            .rounded_t(px(6.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .bg(if tab.active { theme::surface() } else { theme::island() })
                            .text_size(px(12.0))
                            .text_color(if tab.active { theme::text() } else { theme::muted() })
                            .hover(|style| style.bg(theme::surface_hover()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.persist_active_view();
                                if this.session.activate_document(&tab_id) {
                                    this.restore_active_view();
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                }
                            }))
                            .child(file_badge("F", theme::orange()))
                            .child(tab.display_name)
                            .child(div().flex_1())
                            .child(if tab.dirty { "•" } else { "" })
                    })),
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
                            .overflow_scroll()
                            .track_scroll(&self.editor_scroll)
                            .relative()
                            .px(px(18.0))
                            .py(px(10.0))
                            .cursor(CursorStyle::IBeam)
                            .text_size(px(14.0))
                            .text_color(theme::text())
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::editor_mouse_down))
                            .on_mouse_move(cx.listener(Self::editor_mouse_move))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::editor_mouse_up))
                            .child(EditorElement { editor: cx.entity() })
                            .when(self.session.is_empty(), |canvas| {
                                canvas.child(
                                    div()
                                        .absolute()
                                        .top(px(72.0))
                                        .left(px(54.0))
                                        .w(px(330.0))
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(div().text_size(px(18.0)).text_color(theme::text()).child("Open a file or project"))
                                        .child(div().mb_1().text_size(px(12.0)).text_color(theme::subtle()).child("Choose a project to restore its last workspace, or open a file to start here."))
                                        .child(quick_action("Open file", "Ctrl O").on_click(cx.listener(|this, _, window, cx| this.open_file(window, cx))))
                                        .child(quick_action("Open project", "").on_click(cx.listener(|this, _, window, cx| this.open_project(window, cx))))
                                        .child(quick_action("New file", "Ctrl N").on_click(cx.listener(|this, _, window, cx| this.new_document(&New, window, cx)))),
                                )
                            }),
                    ),
            )
    }
}

const fn panel_key(position: PanelPosition) -> u32 {
    match position {
        PanelPosition::Left => 1,
        PanelPosition::Main => 2,
        PanelPosition::Bottom => 3,
        PanelPosition::Right => 4,
    }
}
