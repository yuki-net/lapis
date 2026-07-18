use super::*;

impl Editor {
    pub(super) fn select_execution(&mut self, execution_id: ExecutionId, cx: &mut Context<Self>) {
        self.tasks.selected_execution = Some(execution_id);
        cx.notify();
    }

    pub(super) fn control_selected_task(&mut self, control: TaskControl, cx: &mut Context<Self>) {
        let Some(execution_id) = self.tasks.selected_execution.as_ref() else {
            self.status = "操作する Task がありません".to_owned();
            cx.notify();
            return;
        };
        match self.tasks.session.control(execution_id, control) {
            Ok(()) => self.status = "Task へ操作を送信しました".to_owned(),
            Err(error) => self.status = format!("Task 操作失敗: {error}"),
        }
        cx.notify();
    }

    pub(super) fn reply_to_selected_task(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .filter(|text| !text.is_empty())
        else {
            self.status = "回答をクリップボードへコピーしてください".to_owned();
            cx.notify();
            return;
        };
        self.control_selected_task(TaskControl::Reply { text }, cx);
    }
}
