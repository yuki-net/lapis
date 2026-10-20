use super::*;

impl Editor {
    pub(crate) fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub(crate) fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    pub(crate) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
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

    pub(crate) fn previous_boundary(&self, offset: usize) -> usize {
        offset.saturating_sub(1)
    }

    pub(crate) fn next_boundary(&self, offset: usize) -> usize {
        offset.saturating_add(1).min(self.session.len_chars())
    }

    pub(crate) fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    pub(crate) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub(crate) fn offset_from_utf16(&self, offset: usize) -> usize {
        self.session
            .utf16_offset_to_char(offset)
            .unwrap_or_else(|_| self.session.len_chars())
    }

    pub(crate) fn offset_to_utf16(&self, offset: usize) -> usize {
        self.session
            .char_to_utf16_offset(offset)
            .unwrap_or_default()
    }

    pub(crate) fn replace_selection(&mut self, text: &str, cx: &mut Context<Self>) {
        let range = self.selected_range.clone();
        if let Err(error) = self.edit(range.clone(), text, cx) {
            self.status = format!("編集失敗: {error}");
            cx.notify();
            return;
        }
        let cursor = range.start + text.chars().count();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub(crate) fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = self.previous_boundary(cursor)..cursor;
        }
        self.replace_selection("", cx);
    }

    pub(crate) fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = cursor..self.next_boundary(cursor);
        }
        self.replace_selection("", cx);
    }

    pub(crate) fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    pub(crate) fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    pub(crate) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    pub(crate) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    pub(crate) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.session.len_chars();
        self.selection_reversed = false;
        cx.notify();
    }

    pub(crate) fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        if let Ok(mut position) = self.session.char_to_position(cursor) {
            position.utf16_column = 0;
            if let Ok(start) = self.session.position_to_char(position) {
                self.move_to(start, cx);
            }
        }
    }

    pub(crate) fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        if let Ok(position) = self.session.char_to_position(cursor)
            && let Some(line) = self.session.line(position.line as usize)
        {
            let visible = line.trim_end_matches(['\r', '\n']).chars().count();
            let mut start = position;
            start.utf16_column = 0;
            if let Ok(line_start) = self.session.position_to_char(start) {
                self.move_to(line_start + visible, cx);
            }
        }
    }

    pub(crate) fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.replace_selection("\n", cx);
    }

    pub(crate) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text, cx);
        }
    }

    pub(crate) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty()
            && let Ok(text) = self.session.slice_chars(self.selected_range.clone())
        {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty()
            && let Ok(text) = self.session.slice_chars(self.selected_range.clone())
        {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
        self.replace_selection("", cx);
    }

    pub(crate) fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.document_id.clone() else {
            return;
        };
        self.submit(
            true,
            move |client| {
                client.editor.activate_document(&id);
                client.editor.undo();
                Ok(())
            },
            cx,
        );
    }
    pub(crate) fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.document_id.clone() else {
            return;
        };
        self.submit(
            true,
            move |client| {
                client.editor.activate_document(&id);
                client.editor.redo();
                Ok(())
            },
            cx,
        );
    }
    pub(crate) fn find(&mut self, _: &Find, _: &mut Window, cx: &mut Context<Self>) {
        let query = self
            .selected_text()
            .or_else(|| cx.read_from_clipboard().and_then(|item| item.text()));
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            self.search_matches = self.session.find(&query, true);
            if let Some(range) = self.search_matches.first() {
                self.selected_range = range.clone();
            }
            cx.notify();
        }
    }
    pub(crate) fn find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(range) = self
            .search_matches
            .iter()
            .find(|range| range.start > self.selected_range.start)
            .or_else(|| self.search_matches.first())
        {
            self.selected_range = range.clone();
            cx.notify();
        }
    }
    pub(crate) fn find_workspace(
        &mut self,
        _: &FindWorkspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(query) = self
            .selected_text()
            .or_else(|| cx.read_from_clipboard().and_then(|item| item.text()))
        {
            cx.emit(EditorEvent::WorkspaceSearch(query));
        }
    }
    pub(crate) fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Completion(false));
    }
    pub(crate) fn go_to_definition(
        &mut self,
        _: &GoToDefinition,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(EditorEvent::Completion(true));
    }
}
