use super::*;

pub(super) struct EditorElement {
    pub(super) editor: Entity<Editor>,
}

#[derive(Clone)]
pub(super) struct EditorLineLayout {
    pub(super) start_char: usize,
    pub(super) line: ShapedLine,
    pub(super) origin: Point<Pixels>,
}

pub(super) struct EditorPrepaint {
    lines: Vec<EditorLineLayout>,
    search_highlights: Vec<PaintQuad>,
    selection: Vec<PaintQuad>,
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
        let mut cursor_line = 0;
        let mut cursor_byte = 0;
        let mut offset = 0;

        for line_index in 0..editor.session.len_lines().max(1) {
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
            let cursor = editor.cursor_offset();
            if cursor >= offset
                && (cursor <= offset + visible_chars
                    || line_index + 1 == editor.session.len_lines())
            {
                cursor_line = line_index;
                cursor_byte =
                    byte_for_char(&text, cursor.saturating_sub(offset).min(visible_chars));
            }
            push_range_quad(
                &mut selection,
                &shaped,
                origin,
                offset,
                visible_chars,
                &editor.selected_range,
                rgba(0x6366f166),
            );
            for range in &editor.search.matches {
                push_range_quad(
                    &mut search_highlights,
                    &shaped,
                    origin,
                    offset,
                    visible_chars,
                    range,
                    rgba(0xeab30835),
                );
            }
            lines.push(EditorLineLayout {
                start_char: offset,
                line: shaped,
                origin,
            });
            offset += full_char_len;
        }

        let line = &lines[cursor_line].line;
        let cursor_x = line.x_for_index(cursor_byte);
        let cursor_y = bounds.top() + line_height * cursor_line;
        let cursor = fill(
            Bounds::new(
                point(bounds.left() + cursor_x, cursor_y),
                size(px(2.0), line_height),
            ),
            rgba(0x7dd3fcff),
        );
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
            window.paint_quad(prepaint.cursor.clone());
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
        let layout = self.last_line_layouts.get(position.line as usize)?;
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
