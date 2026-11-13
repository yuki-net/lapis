//! ACP の `session/update` を Lapis の `TaskEvent` へ変換する。
//!
//! ここは ACP 固有の型を core へ漏らさないための境界で、`task` の値だけを返す。

use agent_client_protocol::v1::schema::{
    ContentBlock, SessionUpdate, ToolCallContent, ToolCallStatus, ToolKind,
};
use task::TaskEvent;

/// 1件の `session/update` を表示用イベントへ変換する。表示に意味がない更新は `None`。
pub fn session_update_event(update: &SessionUpdate) -> Option<TaskEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => text_event(chunk.content.clone(), |text| {
            TaskEvent::MessageDelta { text }
        }),
        SessionUpdate::AgentThoughtChunk(chunk) => text_event(chunk.content.clone(), |text| {
            TaskEvent::ReasoningDelta { text }
        }),
        SessionUpdate::ToolCall(call) => Some(TaskEvent::MessageDelta {
            text: format!("\n[{}] {}\n", kind_label(call.kind), call.title),
        }),
        SessionUpdate::ToolCallUpdate(update) => {
            let status = update.fields.status?;
            let label = match status {
                ToolCallStatus::Completed => "完了",
                ToolCallStatus::Failed => "失敗",
                ToolCallStatus::InProgress | ToolCallStatus::Pending => return None,
                _ => return None,
            };
            let title = update
                .fields
                .title
                .clone()
                .or_else(|| update.fields.name.clone())
                .unwrap_or_else(|| "tool".to_owned());
            let output = update
                .fields
                .content
                .as_ref()
                .and_then(|content| first_text(content))
                .unwrap_or_default();
            Some(TaskEvent::MessageDelta {
                text: if output.is_empty() {
                    format!("[{label}] {title}\n")
                } else {
                    format!("[{label}] {title}\n{output}\n")
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

fn text_event(content: ContentBlock, event: impl FnOnce(String) -> TaskEvent) -> Option<TaskEvent> {
    match content {
        ContentBlock::Text(text) if !text.text.is_empty() => Some(event(text.text)),
        _ => None,
    }
}

fn first_text(content: &[ToolCallContent]) -> Option<String> {
    content.iter().find_map(|item| match item {
        ToolCallContent::Content(text) => match &text.content {
            ContentBlock::Text(value) => Some(value.text.trim().to_owned()),
            _ => None,
        },
        _ => None,
    })
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

    #[test]
    fn agent_message_chunk_becomes_message_delta() {
        let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new("pong"),
        )));
        assert_eq!(
            session_update_event(&update),
            Some(TaskEvent::MessageDelta {
                text: "pong".to_owned()
            })
        );
    }

    #[test]
    fn agent_thought_chunk_becomes_reasoning_delta() {
        let update = SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new("thinking"),
        )));
        assert_eq!(
            session_update_event(&update),
            Some(TaskEvent::ReasoningDelta {
                text: "thinking".to_owned()
            })
        );
    }

    #[test]
    fn empty_chunks_are_ignored() {
        let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new(""),
        )));
        assert_eq!(session_update_event(&update), None);
    }

    #[test]
    fn tool_call_reports_kind_and_title() {
        let update =
            SessionUpdate::ToolCall(ToolCall::new("call-1", "read README").kind(ToolKind::Read));
        assert_eq!(
            session_update_event(&update),
            Some(TaskEvent::MessageDelta {
                text: "\n[read] read README\n".to_owned()
            })
        );
    }

    #[test]
    fn completed_tool_call_includes_output() {
        let update = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "call-1",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .title("git status")
                .content(vec![ToolCallContent::Content(ToolContent::new(
                    ContentBlock::Text(TextContent::new("clean")),
                ))]),
        ));
        assert_eq!(
            session_update_event(&update),
            Some(TaskEvent::MessageDelta {
                text: "[完了] git status\nclean\n".to_owned()
            })
        );
    }

    #[test]
    fn in_progress_tool_updates_are_not_displayed() {
        let update = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "call-1",
            ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
        ));
        assert_eq!(session_update_event(&update), None);
    }
}
