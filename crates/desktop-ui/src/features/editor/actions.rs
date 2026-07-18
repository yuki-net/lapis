use super::*;

impl Editor {
    pub(super) fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub(super) fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    pub(super) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
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

    pub(super) fn previous_boundary(&self, offset: usize) -> usize {
        offset.saturating_sub(1)
    }

    pub(super) fn next_boundary(&self, offset: usize) -> usize {
        offset.saturating_add(1).min(self.session.len_chars())
    }

    pub(super) fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    pub(super) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub(super) fn offset_from_utf16(&self, offset: usize) -> usize {
        self.session
            .utf16_offset_to_char(offset)
            .unwrap_or_else(|_| self.session.len_chars())
    }

    pub(super) fn offset_to_utf16(&self, offset: usize) -> usize {
        self.session
            .char_to_utf16_offset(offset)
            .unwrap_or_default()
    }

    pub(super) fn replace_selection(&mut self, text: &str, cx: &mut Context<Self>) {
        let range = self.selected_range.clone();
        if let Err(error) = self.session.replace_range(range.clone(), text) {
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

    pub(super) fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = self.previous_boundary(cursor)..cursor;
        }
        self.replace_selection("", cx);
    }

    pub(super) fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            self.selected_range = cursor..self.next_boundary(cursor);
        }
        self.replace_selection("", cx);
    }

    pub(super) fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    pub(super) fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    pub(super) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    pub(super) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.session.len_chars();
        self.selection_reversed = false;
        cx.notify();
    }

    pub(super) fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        if let Ok(mut position) = self.session.char_to_position(cursor) {
            position.utf16_column = 0;
            if let Ok(start) = self.session.position_to_char(position) {
                self.move_to(start, cx);
            }
        }
    }

    pub(super) fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.replace_selection("\n", cx);
    }

    pub(super) fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text, cx);
        }
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty()
            && let Ok(text) = self.session.slice_chars(self.selected_range.clone())
        {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    pub(super) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty()
            && let Ok(text) = self.session.slice_chars(self.selected_range.clone())
        {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
        self.replace_selection("", cx);
    }

    pub(super) fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.session.undo() {
            self.move_to(self.cursor_offset().min(self.session.len_chars()), cx);
            self.status = "元に戻しました".to_owned();
        }
    }

    pub(super) fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.session.redo() {
            self.move_to(self.cursor_offset().min(self.session.len_chars()), cx);
            self.status = "やり直しました".to_owned();
        }
    }

    pub(super) fn find(&mut self, _: &Find, _: &mut Window, cx: &mut Context<Self>) {
        let selected = (!self.selected_range.is_empty())
            .then(|| self.session.slice_chars(self.selected_range.clone()).ok())
            .flatten();
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(query) = selected.or(clipboard).filter(|value| !value.is_empty()) else {
            self.status = "検索語を選択するかクリップボードへコピーしてください".to_owned();
            cx.notify();
            return;
        };
        self.search.query = query;
        self.search.matches = self.session.find(&self.search.query, true);
        self.search.current_match = 0;
        self.select_tool(ViewId::new(id::VIEW_SEARCH), cx);
        if let Some(range) = self.search.matches.first().cloned() {
            self.selected_range = range;
        }
        self.status = format!("検索: {}件", self.search.matches.len());
        cx.notify();
    }

    pub(super) fn find_workspace(
        &mut self,
        _: &FindWorkspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = (!self.selected_range.is_empty())
            .then(|| self.session.slice_chars(self.selected_range.clone()).ok())
            .flatten();
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(query) = selected.or(clipboard).filter(|value| !value.is_empty()) else {
            self.status = "検索語を選択するかクリップボードへコピーしてください".to_owned();
            cx.notify();
            return;
        };
        let Some(root) = self.session.workspace_root().map(ToOwned::to_owned) else {
            self.status = "Workspace を開いてください".to_owned();
            cx.notify();
            return;
        };
        self.feature_registry
            .activate(ActivationEvent::CommandInvoked(
                id::COMMAND_FIND_WORKSPACE.into(),
            ));
        self.search.workspace.start(root, query);
        self.select_tool(ViewId::new(id::VIEW_SEARCH), cx);
        self.status = "Workspace を検索中…".to_owned();
        cx.notify();
    }

    pub(super) fn find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.matches.is_empty() {
            self.find(&Find, window, cx);
            return;
        }
        self.search.current_match = (self.search.current_match + 1) % self.search.matches.len();
        self.selected_range = self.search.matches[self.search.current_match].clone();
        self.status = format!(
            "検索: {}/{}",
            self.search.current_match + 1,
            self.search.matches.len()
        );
        cx.notify();
    }

    pub(super) fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        let Ok(position) = self.session.char_to_position(self.cursor_offset()) else {
            return;
        };
        match self.problems.lsp.request_completion(
            &self.session,
            LspPosition {
                line: position.line,
                utf16_column: position.utf16_column,
            },
        ) {
            Ok(receiver) => {
                self.problems.completion_receiver = Some(receiver);
                self.status = "補完を取得中…".to_owned();
            }
            Err(error) => self.status = format!("補完失敗: {error}"),
        }
        cx.notify();
    }

    pub(super) fn go_to_definition(
        &mut self,
        _: &GoToDefinition,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(position) = self.session.char_to_position(self.cursor_offset()) else {
            return;
        };
        match self.problems.lsp.request_definition(
            &self.session,
            LspPosition {
                line: position.line,
                utf16_column: position.utf16_column,
            },
        ) {
            Ok(receiver) => {
                self.problems.definition_receiver = Some(receiver);
                self.status = "定義を検索中…".to_owned();
            }
            Err(error) => self.status = format!("定義移動失敗: {error}"),
        }
        cx.notify();
    }

    pub(super) fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.shell.command_palette_open = false;
        self.open_file(window, cx);
    }

    pub(super) fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.shell.command_palette_open = false;
        self.save_file(window, cx);
    }

    pub(super) fn new_document(&mut self, _: &New, _: &mut Window, cx: &mut Context<Self>) {
        self.session.new_document();
        self.selected_range = 0..0;
        self.search.matches.clear();
        self.status = "新しいドキュメント".to_owned();
        self.shell.command_palette_open = false;
        cx.notify();
    }

    pub(super) fn show_commands(
        &mut self,
        _: &ShowCommands,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_quick_search(window, cx);
    }

    pub(super) fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .shell
            .side_panel
            .as_ref()
            .is_some_and(|view| view.as_str() == id::VIEW_COMMAND_SEARCH)
        {
            self.close_quick_search(window, cx);
            return;
        }
        if self.shell.command_palette_open {
            self.shell.command_palette_open = false;
            cx.notify();
        }
    }

    pub(super) fn open_quick_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let provider = CommandSearchProvider::from_registry(
            &self.feature_registry,
            &self.locale,
            &self.keymap,
        );
        let focus = self.quick_search.read(cx).focus_handle();
        let _ = self
            .quick_search
            .update(cx, |search, cx| search.open(provider, cx));
        self.shell.command_palette_open = false;
        self.shell.side_panel = Some(ViewId::new(id::VIEW_COMMAND_SEARCH));
        self.refresh_feature_activation();
        self.double_shift.reset();
        window.focus(&focus);
        cx.notify();
    }

    fn close_quick_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell.side_panel = None;
        self.refresh_feature_activation();
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn handle_quick_search_event(
        &mut self,
        event: QuickSearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            QuickSearchEvent::Execute(command) => {
                self.shell.side_panel = None;
                self.execute_command(command, window, cx);
            }
            QuickSearchEvent::Dismiss => self.close_quick_search(window, cx),
        }
    }

    pub(super) fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .double_shift
            .modifiers_changed(event.modifiers, Instant::now())
        {
            self.open_quick_search(window, cx);
        }
    }

    pub(super) fn normal_key_down(
        &mut self,
        _: &KeyDownEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.double_shift.normal_key_pressed();
    }
}
