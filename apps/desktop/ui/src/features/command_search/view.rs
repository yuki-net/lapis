use std::{
    ops::Range,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, LayoutId, Modifiers, MouseButton,
    MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, relative, size,
};

use crate::{
    components::{ScrollAxis, ScrollableElement},
    extension_ui::CommandId,
    features::command_search::provider::{CommandSearchProvider, SearchItem, SearchProvider},
    theme,
};

actions!(
    quick_search,
    [
        SearchBackspace,
        SearchDelete,
        SearchLeft,
        SearchRight,
        SearchSelectAll,
        SearchHome,
        SearchEnd,
        SearchPrevious,
        SearchNext,
        SearchConfirm,
        SearchDismiss,
    ]
);

const DOUBLE_SHIFT_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub(crate) struct DoubleShiftDetector {
    shift_down: bool,
    first_press: Option<Instant>,
}

impl DoubleShiftDetector {
    pub(crate) fn modifiers_changed(&mut self, modifiers: Modifiers, now: Instant) -> bool {
        let only_shift = modifiers.shift
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.platform
            && !modifiers.function;
        let rising_edge = only_shift && !self.shift_down;
        self.shift_down = modifiers.shift;
        if !rising_edge {
            return false;
        }

        if let Some(first_press) = self.first_press
            && now.saturating_duration_since(first_press) <= DOUBLE_SHIFT_INTERVAL
        {
            self.first_press = None;
            return true;
        }
        self.first_press = Some(now);
        false
    }

    pub(crate) fn normal_key_pressed(&mut self) {
        self.first_press = None;
    }

    pub(crate) fn reset(&mut self) {
        self.shift_down = false;
        self.first_press = None;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuickSearchEvent {
    Execute(CommandId),
    Dismiss,
}

type QuickSearchEventHandler = dyn Fn(QuickSearchEvent, &mut Window, &mut Context<QuickSearch>);

pub(crate) struct QuickSearch {
    focus_handle: FocusHandle,
    query: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    provider: CommandSearchProvider,
    matches: Vec<SearchItem>,
    selected_index: usize,
    on_event: Box<QuickSearchEventHandler>,
}

impl QuickSearch {
    pub(crate) fn new(
        cx: &mut Context<Self>,
        on_event: impl Fn(QuickSearchEvent, &mut Window, &mut Context<QuickSearch>) + 'static,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            query: String::new(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            provider: CommandSearchProvider::default(),
            matches: Vec::new(),
            selected_index: 0,
            on_event: Box::new(on_event),
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub(crate) fn open(&mut self, provider: CommandSearchProvider, cx: &mut Context<Self>) {
        self.provider = provider;
        self.query.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.refresh_matches();
        cx.notify();
    }

    fn refresh_matches(&mut self) {
        self.matches = self.provider.search(&self.query);
        self.selected_index = self
            .selected_index
            .min(self.matches.len().saturating_sub(1));
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

    fn previous_boundary(&self, offset: usize) -> usize {
        self.query[..offset]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.query[offset..]
            .char_indices()
            .nth(1)
            .map(|(index, _)| offset + index)
            .unwrap_or(self.query.len())
    }

    fn offset_from_utf16(&self, target: usize) -> usize {
        byte_offset_from_utf16(&self.query, target)
    }

    fn offset_to_utf16(&self, target: usize) -> usize {
        self.query[..target].encode_utf16().count()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn clamp_range(&self, range: Range<usize>) -> Range<usize> {
        clamp_query_range(&self.query, range)
    }

    fn backspace(&mut self, _: &SearchBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = self.previous_boundary(cursor)..cursor;
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &SearchDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = cursor..self.next_boundary(cursor);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn left(&mut self, _: &SearchLeft, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn right(&mut self, _: &SearchRight, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn select_all(&mut self, _: &SearchSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.query.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &SearchHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &SearchEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.query.len(), cx);
    }

    fn previous(&mut self, _: &SearchPrevious, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = previous_selection(self.selected_index, self.matches.len()) {
            self.selected_index = index;
            cx.notify();
        }
    }

    fn next(&mut self, _: &SearchNext, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = next_selection(self.selected_index, self.matches.len()) {
            self.selected_index = index;
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &SearchConfirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = selected_command(&self.matches, self.selected_index) {
            (self.on_event)(QuickSearchEvent::Execute(command), window, cx);
        }
    }

    fn dismiss(&mut self, _: &SearchDismiss, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_event)(QuickSearchEvent::Dismiss, window, cx);
    }

    fn execute_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.matches.get(index) {
            (self.on_event)(QuickSearchEvent::Execute(item.command.clone()), window, cx);
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let index = self.index_for_mouse_position(event.position);
        self.move_to(index, cx);
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (&self.last_bounds, &self.last_layout) else {
            return self.query.len();
        };
        if position.x <= bounds.left() {
            return 0;
        }
        if position.x >= bounds.right() {
            return self.query.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }
}

fn byte_offset_from_utf16(text: &str, target: usize) -> usize {
    let mut utf8 = 0;
    let mut utf16 = 0;
    for ch in text.chars() {
        if utf16 >= target {
            break;
        }
        utf8 += ch.len_utf8();
        utf16 += ch.len_utf16();
    }
    utf8
}

fn previous_selection(current: usize, len: usize) -> Option<usize> {
    (len > 0).then(|| current.checked_sub(1).unwrap_or(len - 1))
}

fn next_selection(current: usize, len: usize) -> Option<usize> {
    (len > 0).then(|| (current + 1) % len)
}

fn selected_command(matches: &[SearchItem], selected_index: usize) -> Option<CommandId> {
    matches.get(selected_index).map(|item| item.command.clone())
}

fn clamp_query_range(query: &str, range: Range<usize>) -> Range<usize> {
    let len = query.len();
    let mut start = range.start.min(len);
    let mut end = range.end.min(len).max(start);
    while start > 0 && !query.is_char_boundary(start) {
        start -= 1;
    }
    while end < len && !query.is_char_boundary(end) {
        end += 1;
    }
    start..end
}

impl EntityInputHandler for QuickSearch {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.query[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw_range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.clamp_range(raw_range);
        let new_text = new_text.replace(['\r', '\n'], " ");
        self.query.replace_range(range.clone(), &new_text);
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.refresh_matches();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw_range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.clamp_range(raw_range);
        let new_text = new_text.replace(['\r', '\n'], " ");
        self.query.replace_range(range.clone(), &new_text);
        self.marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                range.start + byte_offset_from_utf16(&new_text, selected.start)
                    ..range.start + byte_offset_from_utf16(&new_text, selected.end)
            })
            .unwrap_or(range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.refresh_matches();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

struct SearchTextElement {
    input: Entity<QuickSearch>,
}

struct SearchTextPrepaint {
    line: ShapedLine,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for SearchTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SearchTextElement {
    type RequestLayoutState = ();
    type PrepaintState = SearchTextPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = px(22.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> SearchTextPrepaint {
        let input = self.input.read(cx);
        let style = window.text_style();
        let display_text: gpui::SharedString = if input.query.is_empty() {
            "Search commands…".into()
        } else {
            input.query.clone().into()
        };
        let color = if input.query.is_empty() {
            theme::subtle().into()
        } else {
            style.color
        };
        let base_run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked) = &input.marked_range {
            vec![
                TextRun {
                    len: marked.start,
                    ..base_run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..base_run.clone()
                },
                TextRun {
                    len: display_text.len() - marked.end,
                    ..base_run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect::<Vec<_>>()
        } else {
            vec![base_run]
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);
        let selection = (!input.selected_range.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(
                        bounds.left() + line.x_for_index(input.selected_range.start),
                        bounds.top(),
                    ),
                    point(
                        bounds.left() + line.x_for_index(input.selected_range.end),
                        bounds.bottom(),
                    ),
                ),
                theme::search_selection(),
            )
        });
        let cursor = input.selected_range.is_empty().then(|| {
            fill(
                Bounds::new(
                    point(
                        bounds.left() + line.x_for_index(input.cursor_offset()),
                        bounds.top(),
                    ),
                    size(px(1.0), bounds.size.height),
                ),
                theme::text(),
            )
        });
        SearchTextPrepaint {
            line,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut SearchTextPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        prepaint
            .line
            .paint(bounds.origin, px(22.0), window, cx)
            .ok();
        if focus.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        let line = prepaint.line.clone();
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for QuickSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.matches.iter().cloned().enumerate().collect::<Vec<_>>();
        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .key_context("QuickSearch")
            .track_focus(&self.focus_handle())
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::previous))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .child(
                div()
                    .m_2()
                    .mb_1()
                    .h(px(36.0))
                    .px_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor(CursorStyle::IBeam)
                    .text_size(px(13.0))
                    .text_color(theme::text())
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .child(div().text_color(theme::muted()).child("S"))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .child(SearchTextElement { input: cx.entity() }),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(10.0))
                    .text_color(theme::subtle())
                    .child(format!(
                        "{} commands ﾂｷ 竊鯛・ select ﾂｷ Enter run",
                        rows.len()
                    )),
            )
            .child(
                div()
                    .id("quick-search-results")
                    .h(px(0.0))
                    .min_h(px(0.0))
                    .flex_1()
                    .scrollable(ScrollAxis::Vertical)
                    .px_2()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(rows.is_empty(), |results| {
                        results.child(
                            div()
                                .p_3()
                                .text_size(px(12.0))
                                .text_color(theme::subtle())
                                .child("No matching commands"),
                        )
                    })
                    .children(rows.into_iter().map(|(index, item)| {
                        let command_label = item.command.as_str().to_owned();
                        div()
                            .id(("quick-search-result", index))
                            .px_2()
                            .py_2()
                            .rounded(px(6.0))
                            .bg(if index == self.selected_index {
                                theme::surface_active()
                            } else {
                                theme::island()
                            })
                            .hover(|style| style.bg(theme::surface_hover()))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap_2()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.execute_index(index, window, cx);
                            }))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(theme::text())
                                            .child(item.title),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.0))
                                            .text_color(theme::subtle())
                                            .child(command_label),
                                    ),
                            )
                            .when(!item.shortcut.is_empty(), |row| {
                                row.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(theme::muted())
                                        .child(item.shortcut),
                                )
                            })
                    })),
            )
    }
}

impl Focusable for QuickSearch {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_item(command: &str) -> SearchItem {
        SearchItem {
            command: CommandId::new(command),
            title: command.to_owned(),
            shortcut: String::new(),
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn double_shift_triggers_once_within_interval() {
        let start = Instant::now();
        let mut detector = DoubleShiftDetector::default();
        assert!(!detector.modifiers_changed(shift(), start));
        assert!(!detector.modifiers_changed(Modifiers::default(), start));
        assert!(detector.modifiers_changed(shift(), start + Duration::from_millis(499)));
        assert!(!detector.modifiers_changed(shift(), start + Duration::from_millis(500)));
    }

    #[test]
    fn double_shift_ignores_hold_and_intervening_key() {
        let start = Instant::now();
        let mut detector = DoubleShiftDetector::default();
        assert!(!detector.modifiers_changed(shift(), start));
        assert!(!detector.modifiers_changed(shift(), start + Duration::from_millis(50)));
        detector.normal_key_pressed();
        assert!(!detector.modifiers_changed(Modifiers::default(), start));
        assert!(!detector.modifiers_changed(shift(), start + Duration::from_millis(100)));
    }

    #[test]
    fn double_shift_restarts_after_timeout_and_rejects_modifier_chords() {
        let start = Instant::now();
        let mut detector = DoubleShiftDetector::default();
        assert!(!detector.modifiers_changed(shift(), start));
        assert!(!detector.modifiers_changed(Modifiers::default(), start));
        assert!(!detector.modifiers_changed(shift(), start + Duration::from_millis(501)));
        assert!(!detector.modifiers_changed(Modifiers::default(), start));
        assert!(!detector.modifiers_changed(
            Modifiers {
                shift: true,
                control: true,
                ..Modifiers::default()
            },
            start + Duration::from_millis(600),
        ));
    }

    #[test]
    fn result_selection_wraps_with_up_and_down() {
        assert_eq!(previous_selection(0, 3), Some(2));
        assert_eq!(previous_selection(2, 3), Some(1));
        assert_eq!(next_selection(2, 3), Some(0));
        assert_eq!(next_selection(0, 3), Some(1));
        assert_eq!(previous_selection(0, 0), None);
        assert_eq!(next_selection(0, 0), None);
    }

    #[test]
    fn clamp_range_handles_out_of_bounds_and_empty_query() {
        assert_eq!(clamp_query_range("", 0..18), 0..0);
        assert_eq!(clamp_query_range("", 10..20), 0..0);
        assert_eq!(clamp_query_range("hello", 0..10), 0..5);
        assert_eq!(clamp_query_range("hello", 2..4), 2..4);
    }

    #[test]
    fn enter_executes_the_currently_selected_command() {
        let matches = vec![search_item("command.first"), search_item("command.second")];
        assert_eq!(
            selected_command(&matches, 1).unwrap().as_str(),
            "command.second"
        );
        assert!(selected_command(&matches, 2).is_none());
    }
}
