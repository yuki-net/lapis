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
                .min(self.last_line_layouts.len() - 1)
        };
        let layout = &self.last_line_layouts[line_index];
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

    pub(super) fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.session.choose_workspace() {
            Ok(DocumentAction::Completed) => {
                self.selected_range = 0..0;
                self.status = "Workspaceを開きました".to_owned();
                window.focus(&self.focus_handle);
                cx.notify();
            }
            Ok(DocumentAction::Cancelled) => {}
            Err(error) => self.status = format!("読み込み失敗: {error}"),
        }
    }

    pub(super) fn save_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
}
