use super::*;

impl Editor {
    pub(super) fn persist_active_view(&mut self) {
        let offset = self.editor_scroll.offset();
        self.session.update_active_view(
            self.selected_range.clone(),
            self.cursor_offset(),
            -f32::from(offset.x),
            -f32::from(offset.y),
        );
    }

    pub(super) fn restore_active_view(&mut self) {
        if let Some(view) = self.session.active_view().cloned() {
            let len = self.session.len_chars();
            let start = view.selection_start.min(len);
            let end = view.selection_end.min(len);
            self.selected_range = start.min(end)..start.max(end);
            self.selection_reversed =
                view.cursor_char == self.selected_range.start && !self.selected_range.is_empty();
            self.editor_scroll
                .scroll_to_top_of_item((view.scroll_y / 24.0).max(0.0) as usize);
        } else {
            self.selected_range = 0..0;
            self.selection_reversed = false;
        }
    }
}
