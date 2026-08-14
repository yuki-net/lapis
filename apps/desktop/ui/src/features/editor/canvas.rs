use super::*;

pub(super) struct EditorElement {
    pub(super) editor: Entity<Editor>,
}

#[derive(Clone)]
pub(super) struct EditorLineLayout {
    pub(super) line_index: usize,
    pub(super) start_char: usize,
    pub(super) line: ShapedLine,
    pub(super) origin: Point<Pixels>,
}

pub(super) struct EditorPrepaint {
    lines: Vec<EditorLineLayout>,
    search_highlights: Vec<PaintQuad>,
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
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
        let editor = self.editor.read(cx);
        let line_count = editor.session.len_lines().max(1);
        let max_chars = (0..line_count)
            .filter_map(|line| editor.session.line(line))
            .map(|line| line.trim_end_matches(['\r', '\n']).chars().count())
            .max()
            .unwrap_or(0);
        let mut style = Style::default();
        style.size.width = px((max_chars as f32 * 9.0 + 40.0).max(600.0)).into();
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
        let text_style = window.text_style();
        let mut lines = Vec::new();
        let mut selection = Vec::new();
        let mut search_highlights = Vec::new();
        let line_height = px(24.0);
        let line_count = editor.session.len_lines().max(1);
        let viewport = editor.editor_scroll.bounds();
        let (first_line, last_line) = if viewport.size.height > px(0.0) {
            let first = (f32::from(viewport.top() - bounds.top()) / 24.0)
                .floor()
                .max(0.0) as usize;
            let last = (f32::from(viewport.bottom() - bounds.top()) / 24.0)
                .ceil()
                .max(0.0) as usize;
            (
                first.saturating_sub(1).min(line_count),
                last.saturating_add(1).min(line_count),
            )
        } else {
            (0, line_count)
        };
        let mut offset = editor.session.line_start_char(first_line).unwrap_or(0);
        let cursor = editor.cursor_offset();
        let cursor_line = editor
            .session
            .char_to_position(cursor)
            .map(|position| position.line as usize)
            .unwrap_or(0)
            .min(line_count.saturating_sub(1));
        let selection_color = theme::editor_selection();
        let search_match_color = theme::editor_search_match();

        for line_index in first_line..last_line {
            let raw = editor.session.line(line_index).unwrap_or_default();
            let full_char_len = raw.chars().count();
            let text = raw.trim_end_matches(['\r', '\n']).to_owned();
            let visible_chars = text.chars().count();
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
            push_range_quad(
                &mut selection,
                &shaped,
                origin,
                offset,
                visible_chars,
                &editor.selected_range,
                selection_color,
            );
            for range in &editor.search.matches {
                push_range_quad(
                    &mut search_highlights,
                    &shaped,
                    origin,
                    offset,
                    visible_chars,
                    range,
                    search_match_color,
                );
            }
            lines.push(EditorLineLayout {
                line_index,
                start_char: offset,
                line: shaped,
                origin,
            });
            offset += full_char_len;
        }

        let cursor = lines
            .iter()
            .find(|layout| layout.line_index == cursor_line)
            .map(|layout| {
                let cursor_line_start = layout.start_char;
                let cursor_byte =
                    byte_for_char(&layout.line.text, cursor.saturating_sub(cursor_line_start));
                let cursor_x = layout.line.x_for_index(cursor_byte);
                fill(
                    Bounds::new(
                        point(layout.origin.x + cursor_x, layout.origin.y),
                        size(px(2.0), line_height),
                    ),
                    theme::editor_cursor(),
                )
            });
        EditorPrepaint {
            lines,
            search_highlights,
            selection,
            cursor,
        }
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
        for highlight in prepaint.search_highlights.drain(..) {
            window.paint_quad(highlight);
        }
        for selection in prepaint.selection.drain(..) {
            window.paint_quad(selection);
        }
        for layout in &prepaint.lines {
            layout.line.paint(layout.origin, px(24.0), window, cx).ok();
        }
        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.clone() {
                window.paint_quad(cursor);
            }
        }
        let layouts = prepaint.lines.clone();
        self.editor.update(cx, |editor, _| {
            editor.last_editor_bounds = Some(bounds);
            editor.last_line_layouts = layouts;
        });
    }
}

fn byte_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

fn push_range_quad(
    output: &mut Vec<PaintQuad>,
    shaped: &ShapedLine,
    origin: Point<Pixels>,
    line_start: usize,
    visible_chars: usize,
    range: &Range<usize>,
    color: gpui::Rgba,
) {
    let start = range.start.max(line_start);
    let end = range.end.min(line_start + visible_chars);
    if start >= end {
        return;
    }
    let start_byte = byte_for_char(&shaped.text, start - line_start);
    let end_byte = byte_for_char(&shaped.text, end - line_start);
    let start_x = shaped.x_for_index(start_byte);
    let end_x = shaped.x_for_index(end_byte);
    output.push(fill(
        Bounds::new(
            point(origin.x + start_x, origin.y),
            size((end_x - start_x).max(px(2.0)), px(24.0)),
        ),
        color,
    ));
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
        self.session.slice_chars(range).ok()
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
        let target = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        let start = target.start;
        if let Err(error) = self.session.replace_range(target, new_text) {
            self.status = format!("入力失敗: {error}");
            cx.notify();
            return;
        }
        let inserted_chars = new_text.chars().count();
        self.marked_range = Some(start..start + inserted_chars);
        self.selected_range = new_selected_range_utf16
            .map(|range| {
                start + char_for_utf16_in_text(new_text, range.start)
                    ..start + char_for_utf16_in_text(new_text, range.end)
            })
            .unwrap_or(start + inserted_chars..start + inserted_chars);
        self.selection_reversed = false;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let position = self.session.char_to_position(range.start).ok()?;
        let layout = self
            .last_line_layouts
            .iter()
            .find(|layout| layout.line_index == position.line as usize)?;
        let local_char = range.start.saturating_sub(layout.start_char);
        let byte = byte_for_char(&layout.line.text, local_char);
        let x = layout.line.x_for_index(byte);
        Some(Bounds::new(
            point(layout.origin.x + x, layout.origin.y),
            size(px(2.0), px(24.0)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

fn char_for_utf16_in_text(text: &str, target: usize) -> usize {
    let mut utf16 = 0usize;
    for (index, ch) in text.chars().enumerate() {
        if utf16 >= target {
            return index;
        }
        utf16 += ch.len_utf16();
    }
    text.chars().count()
}
