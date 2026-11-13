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
    /// 完了を通知済みか。同じ status の再送で二重に表示しない。
    reported: bool,
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
                self.tools.insert(
                    id,
                    ToolState {
                        title: call.title.clone(),
                        kind: call.kind,
                        output: String::new(),
                        reported: false,
                    },
                );
                Some(TaskEvent::MessageDelta {
                    text: format!("\n[{}] {}\n", kind_label(call.kind), call.title),
                })
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.to_string();
                let state = self.tools.entry(id).or_insert_with(|| ToolState {
                    title: "tool".to_owned(),
                    kind: ToolKind::Other,
                    output: String::new(),
                    reported: false,
                });
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
                let status = update.fields.status?;
                let label = match status {
                    ToolCallStatus::Completed => "完了",
                    ToolCallStatus::Failed => "失敗",
                    _ => return None,
                };
                if state.reported {
                    return None;
                }
                state.reported = true;
                Some(TaskEvent::MessageDelta {
                    text: if state.output.is_empty() {
                        format!("[{label}] {}\n", state.title)
                    } else {
                        format!("[{label}] {}\n{}\n", state.title, state.output)
                    },
                })
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
}
