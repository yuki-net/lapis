//! ACP の `session/update` を Lapis の `TaskEvent` へ変換する。
//!
//! ACP v1 の tool call 更新は、変更されたフィールドだけを送る部分更新で届く。
//! `content` と `status` が別の更新で届くこともあるため、`toolCallId` ごとに
//! 状態を集約してから表示用イベントへ変換する。ここは ACP 固有の型を core へ
//! 漏らさないための境界で、`task` の値だけを返す。

use std::collections::HashMap;

use agent_client_protocol::v1::schema::{
    ContentBlock, SessionUpdate, ToolCallContent, ToolCallStatus, ToolKind,
};
use task::TaskEvent;

/// `session/update` を順に受け取り、表示用イベントへ変換する。
#[derive(Debug, Default)]
pub struct UpdateMapper {
    tools: HashMap<String, ToolState>,
}

#[derive(Debug)]
struct ToolState {
    title: String,
    kind: ToolKind,
    /// 直近の `content`。置換であり追記ではない。
    output: String,
    /// 完了（または失敗）を通知済みのラベル。
    completion: Option<&'static str>,
    /// 完了通知を表示済みか。
    reported_completion: bool,
    /// 表示済みの出力。完了通知より後に届いた出力の重複表示を防ぐ。
    reported_output: String,
}

impl ToolState {
    fn new(title: String, kind: ToolKind) -> Self {
        Self {
            title,
            kind,
            output: String::new(),
            completion: None,
            reported_completion: false,
            reported_output: String::new(),
        }
    }

    /// 完了通知本文。未表示の出力があれば含める。
    fn completion_text(&self, label: &str) -> String {
        if self.output.is_empty() {
            format!("[{label}] {}\n", self.title)
        } else {
            format!("[{label}] {}\n{}\n", self.title, self.output)
        }
    }
}

impl UpdateMapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// 1件の `session/update` を表示用イベントへ変換する。表示に意味がない更新は `None`。
    pub fn map(&mut self, update: &SessionUpdate) -> Option<TaskEvent> {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => text_event(chunk.content.clone(), |text| {
                TaskEvent::MessageDelta { text }
            }),
            SessionUpdate::AgentThoughtChunk(chunk) => text_event(chunk.content.clone(), |text| {
                TaskEvent::ReasoningDelta { text }
            }),
            SessionUpdate::ToolCall(call) => {
                let id = call.tool_call_id.to_string();
                // 初回通知にも content と status が含まれ得る。捨てずに状態へ取り込む。
                let mut state = ToolState::new(call.title.clone(), call.kind);
                state.output = collect_text(&call.content);
                state.completion = completion_label(call.status);
                let start = format!("\n[{}] {}\n", kind_label(call.kind), call.title);
                // 初回通知だけで完了している場合は、開始と完了の両方をこの時点で出す。
                let completed = match state.completion {
                    Some(label) => {
                        state.reported_completion = true;
                        state.reported_output.clone_from(&state.output);
                        Some(state.completion_text(label))
                    }
                    None => None,
                };
                self.tools.insert(id, state);
                let mut text = start;
                if let Some(completed) = completed {
                    text.push_str(&completed);
                }
                Some(TaskEvent::MessageDelta { text })
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.to_string();
                let state = self
                    .tools
                    .entry(id)
                    .or_insert_with(|| ToolState::new("tool".to_owned(), ToolKind::Other));
                if let Some(kind) = update.fields.kind {
                    state.kind = kind;
                }
                if let Some(title) = &update.fields.title {
                    state.title.clone_from(title);
                }
                if let Some(content) = &update.fields.content {
                    // content は置換。累積内容を二重に追記しない。
                    state.output = collect_text(content);
                }
                if let Some(label) = update.fields.status.and_then(completion_label)
                    && state.completion.is_none()
                {
                    state.completion = Some(label);
                }
                let Some(label) = state.completion else {
                    // 未完了の更新は表示用イベントを持たない。
                    return None;
                };
                // 完了通知は1度だけ。その後に届いた出力は追記として反映する。
                if !state.reported_completion {
                    state.reported_completion = true;
                    state.reported_output.clone_from(&state.output);
                    return Some(TaskEvent::MessageDelta {
                        text: state.completion_text(label),
                    });
                }
                if state.output == state.reported_output {
                    return None;
                }
                // content は置換なので短縮・同長置換・UTF-8 があり得る。
                // `MessageDelta` は追記専用に合成されるため、置換は次の 2 通りに分ける。
                // - 表示済み出力の後ろへの追加: 差分だけを追記する
                // - それ以外（短縮・同長・内容変更）: 更新であることを明示して全体を出す
                match state.output.strip_prefix(state.reported_output.as_str()) {
                    Some(suffix) => {
                        let added = suffix.trim_start_matches('\n').to_owned();
                        state.reported_output.clone_from(&state.output);
                        Some(TaskEvent::MessageDelta {
                            text: format!("[{label}] {}\n{}\n", state.title, added),
                        })
                    }
                    None => {
                        state.reported_output.clone_from(&state.output);
                        Some(TaskEvent::MessageDelta {
                            text: format!("[{label}] {} (更新)\n{}\n", state.title, state.output),
                        })
                    }
                }
            }
            SessionUpdate::UserMessageChunk(_)
            | SessionUpdate::Plan(_)
            | SessionUpdate::AvailableCommandsUpdate(_)
            | SessionUpdate::CurrentModeUpdate(_) => None,
            _ => None,
        }
    }
}

fn text_event(content: ContentBlock, event: impl FnOnce(String) -> TaskEvent) -> Option<TaskEvent> {
    match content {
        ContentBlock::Text(text) if !text.text.is_empty() => Some(event(text.text)),
        _ => None,
    }
}

/// content 内のテキストを全て順序どおり連結する。空要素は無視する。
fn collect_text(content: &[ToolCallContent]) -> String {
    let parts = content
        .iter()
        .filter_map(|item| match item {
            ToolCallContent::Content(text) => match &text.content {
                ContentBlock::Text(value) if !value.text.trim().is_empty() => {
                    Some(value.text.trim())
                }
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    parts.join("\n")
}

fn kind_label(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "mode",
        _ => "tool",
    }
}

/// 完了・失敗だけを表示ラベルへ変換する。それ以外は表示しない。
fn completion_label(status: ToolCallStatus) -> Option<&'static str> {
    match status {
        ToolCallStatus::Completed => Some("完了"),
        ToolCallStatus::Failed => Some("失敗"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema::{
        Content as ToolContent, ContentChunk, TextContent, ToolCall, ToolCallUpdate,
        ToolCallUpdateFields,
    };

    fn text_chunk(text: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    fn tool_update(
        status: Option<ToolCallStatus>,
        content: Option<Vec<ToolCallContent>>,
    ) -> SessionUpdate {
        let mut fields = ToolCallUpdateFields::new();
        if let Some(content) = content {
            fields = fields.content(content);
        }
        if let Some(status) = status {
            fields = fields.status(status);
        }
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new("call-1", fields))
    }

    fn text_content(text: &str) -> Vec<ToolCallContent> {
        vec![ToolCallContent::Content(ToolContent::new(
            ContentBlock::Text(TextContent::new(text)),
        ))]
    }

    #[test]
    fn agent_message_chunk_becomes_message_delta() {
        let mut mapper = UpdateMapper::new();
        assert_eq!(
            mapper.map(&text_chunk("pong")),
            Some(TaskEvent::MessageDelta {
                text: "pong".to_owned()
            })
        );
    }

    #[test]
    fn agent_thought_chunk_becomes_reasoning_delta() {
        let mut mapper = UpdateMapper::new();
        let update = SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new("thinking"),
        )));
        assert_eq!(
            mapper.map(&update),
            Some(TaskEvent::ReasoningDelta {
                text: "thinking".to_owned()
            })
        );
    }

    #[test]
    fn empty_chunks_are_ignored() {
        let mut mapper = UpdateMapper::new();
        assert_eq!(mapper.map(&text_chunk("")), None);
    }

    #[test]
    fn tool_call_reports_kind_and_title() {
        let mut mapper = UpdateMapper::new();
        let update =
            SessionUpdate::ToolCall(ToolCall::new("call-1", "read README").kind(ToolKind::Read));
        assert_eq!(
            mapper.map(&update),
            Some(TaskEvent::MessageDelta {
                text: "\n[read] read README\n".to_owned()
            })
        );
    }

    #[test]
    fn completed_tool_call_includes_output() {
        let mut mapper = UpdateMapper::new();
        let update = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "call-1",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .title("git status")
                .content(text_content("clean")),
        ));
        assert_eq!(
            mapper.map(&update),
            Some(TaskEvent::MessageDelta {
                text: "[完了] git status\nclean\n".to_owned()
            })
        );
    }

    #[test]
    fn in_progress_tool_updates_are_not_displayed() {
        let mut mapper = UpdateMapper::new();
        assert_eq!(
            mapper.map(&tool_update(Some(ToolCallStatus::InProgress), None)),
            None
        );
    }

    #[test]
    fn output_arriving_before_completion_is_kept() {
        let mut mapper = UpdateMapper::new();
        // content だけが先に届く。
        assert_eq!(
            mapper.map(&tool_update(None, Some(text_content("clean")))),
            None
        );
        // 後から status だけが届く。
        assert_eq!(
            mapper.map(&tool_update(Some(ToolCallStatus::Completed), None)),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool\nclean\n".to_owned()
            })
        );
    }

    #[test]
    fn completion_is_reported_only_once() {
        let mut mapper = UpdateMapper::new();
        let first = mapper.map(&tool_update(Some(ToolCallStatus::Completed), None));
        assert!(first.is_some());
        assert_eq!(
            mapper.map(&tool_update(Some(ToolCallStatus::Completed), None)),
            None
        );
    }

    #[test]
    fn output_arriving_after_completion_is_still_reported() {
        let mut mapper = UpdateMapper::new();
        // status だけが先に届く。
        assert_eq!(
            mapper.map(&tool_update(Some(ToolCallStatus::Completed), None)),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool\n".to_owned()
            })
        );
        // 後から content だけが届いても出力を失わない。
        assert_eq!(
            mapper.map(&tool_update(None, Some(text_content("late")))),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool\nlate\n".to_owned()
            })
        );
        // 同じ内容の再送では二重表示しない。
        assert_eq!(
            mapper.map(&tool_update(None, Some(text_content("late")))),
            None
        );
    }

    #[test]
    fn content_is_replaced_not_appended() {
        let mut mapper = UpdateMapper::new();
        mapper.map(&tool_update(None, Some(text_content("first"))));
        mapper.map(&tool_update(None, Some(text_content("second"))));
        assert_eq!(
            mapper.map(&tool_update(Some(ToolCallStatus::Completed), None)),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool\nsecond\n".to_owned()
            })
        );
    }

    #[test]
    fn every_text_element_is_collected_in_order() {
        let mut mapper = UpdateMapper::new();
        let mut content = vec![ToolCallContent::Content(ToolContent::new(
            ContentBlock::Text(TextContent::new("")),
        ))];
        content.extend(text_content("stdout"));
        content.extend(text_content("stderr"));
        assert_eq!(
            mapper.map(&tool_update(Some(ToolCallStatus::Completed), Some(content))),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool\nstdout\nstderr\n".to_owned()
            })
        );
    }

    #[test]
    fn tool_calls_are_tracked_independently() {
        let mut mapper = UpdateMapper::new();
        mapper.map(&SessionUpdate::ToolCall(
            ToolCall::new("call-a", "read a").kind(ToolKind::Read),
        ));
        mapper.map(&SessionUpdate::ToolCall(
            ToolCall::new("call-b", "read b").kind(ToolKind::Read),
        ));
        let complete_a = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "call-a",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(text_content("a-done")),
        ));
        assert_eq!(
            mapper.map(&complete_a),
            Some(TaskEvent::MessageDelta {
                text: "[完了] read a\na-done\n".to_owned()
            })
        );
    }

    #[test]
    fn initial_tool_call_content_and_status_are_kept() {
        let mut mapper = UpdateMapper::new();
        // 初回通知にだけ出力と完了が含まれるケース。
        assert_eq!(
            mapper.map(&SessionUpdate::ToolCall(
                ToolCall::new("call-1", "read README")
                    .kind(ToolKind::Read)
                    .status(ToolCallStatus::Completed)
                    .content(text_content("first-output")),
            )),
            Some(TaskEvent::MessageDelta {
                text: "\n[read] read README\n[完了] read README\nfirst-output\n".to_owned()
            })
        );
        // 後続の status 再送では二重表示しない（既に完了と出力を表示済み）。
        assert_eq!(
            mapper.map(&tool_update(Some(ToolCallStatus::Completed), None)),
            None
        );
    }

    #[test]
    fn initial_tool_call_completed_without_followup_update_shows_output() {
        let mut mapper = UpdateMapper::new();
        // 後続の ToolCallUpdate が一切届かないケース。
        let event = mapper
            .map(&SessionUpdate::ToolCall(
                ToolCall::new("call-1", "run tests")
                    .kind(ToolKind::Execute)
                    .status(ToolCallStatus::Completed)
                    .content(text_content("all green")),
            ))
            .expect("initial completion must be displayed");
        let text = event.display_text();
        assert!(
            text.contains("[完了]"),
            "completion label missing: {text:?}"
        );
        assert!(text.contains("all green"), "output missing: {text:?}");
    }

    /// content は置換なので、短縮・同長・UTF-8 のいずれでも panic せず更新として示す。
    #[test]
    fn replaced_output_shorter_than_reported_is_safe() {
        let mut mapper = UpdateMapper::new();
        mapper.map(&tool_update(
            Some(ToolCallStatus::Completed),
            Some(text_content("long")),
        ));
        // 以前の出力より短い置換。範囲外スライスにならない。
        assert_eq!(
            mapper.map(&tool_update(None, Some(text_content("x")))),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool (更新)\nx\n".to_owned()
            })
        );
    }

    #[test]
    fn same_length_replacement_is_fully_reported() {
        let mut mapper = UpdateMapper::new();
        mapper.map(&tool_update(
            Some(ToolCallStatus::Completed),
            Some(text_content("aaaa")),
        ));
        assert_eq!(
            mapper.map(&tool_update(None, Some(text_content("bbbb")))),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool (更新)\nbbbb\n".to_owned()
            })
        );
    }

    #[test]
    fn utf8_replacement_is_safe_and_complete() {
        let mut mapper = UpdateMapper::new();
        mapper.map(&tool_update(
            Some(ToolCallStatus::Completed),
            Some(text_content("日本語")),
        ));
        // 文字境界を跨ぐ比較でも panic しない。
        assert_eq!(
            mapper.map(&tool_update(None, Some(text_content("確認済み")))),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool (更新)\n確認済み\n".to_owned()
            })
        );
    }

    #[test]
    fn shortened_non_prefix_replacement_does_not_reappend_old_text() {
        let mut mapper = UpdateMapper::new();
        mapper.map(&tool_update(
            Some(ToolCallStatus::Completed),
            Some(text_content("abcdef")),
        ));
        // "abcdef" -> "abcde" は短縮（接頭辞ではない）。更新として全体を出し、
        // 旧内容を再追記しない。
        assert_eq!(
            mapper.map(&tool_update(None, Some(text_content("abcde")))),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool (更新)\nabcde\n".to_owned()
            })
        );
        // 同じ内容の再送では二重表示しない。
        assert_eq!(
            mapper.map(&tool_update(None, Some(text_content("abcde")))),
            None
        );
    }

    #[test]
    fn appended_output_is_reported_as_a_suffix_only() {
        let mut mapper = UpdateMapper::new();
        mapper.map(&tool_update(
            Some(ToolCallStatus::Completed),
            Some(text_content("first")),
        ));
        let content = vec![
            ToolCallContent::Content(ToolContent::new(ContentBlock::Text(TextContent::new(
                "first",
            )))),
            ToolCallContent::Content(ToolContent::new(ContentBlock::Text(TextContent::new(
                "second",
            )))),
        ];
        // 接頭辞が一致するときだけ差分を出す。
        assert_eq!(
            mapper.map(&tool_update(None, Some(content))),
            Some(TaskEvent::MessageDelta {
                text: "[完了] tool\nsecond\n".to_owned()
            })
        );
    }
}
