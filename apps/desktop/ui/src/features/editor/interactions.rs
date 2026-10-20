use super::*;

impl Editor {
    pub(super) fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.session.is_empty() || self.last_line_layouts.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_editor_bounds else {
            return self.cursor_offset();
        };
        let line_index = if position.y <= bounds.top() {
            0
        } else {
            ((f32::from(position.y - bounds.top()) / 24.0).floor() as usize)
                .min(self.session.len_lines().saturating_sub(1))
        };
        let Some(layout) = self
            .last_line_layouts
            .iter()
            .min_by_key(|layout| layout.line_index.abs_diff(line_index))
        else {
            return self.cursor_offset();
        };
        let byte = layout
            .line
            .closest_index_for_x(position.x - layout.origin.x);
        layout.start_char
            + layout.line.text[..byte.min(layout.line.text.len())]
                .chars()
                .count()
    }

    pub(super) fn editor_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        window.focus(&self.focus_handle);
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    pub(super) fn editor_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting && event.dragging() {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    pub(super) fn editor_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.is_selecting = false;
    }
}
