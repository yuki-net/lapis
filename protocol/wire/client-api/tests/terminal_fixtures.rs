//! Terminal 専用 wire の fixture。
//!
//! 要求は Workspace と Workspace 相対 path だけを運ぶ。Mobile 側の fixture とは
//! 独立しているため、`wire_fixtures.rs` の Mobile 同期テストと同じ file に置かない。

use client_api::{
    RequestBody, RequestEnvelope, RequestId, ResponseBody, ResponseEnvelope, TerminalId,
    TerminalRequest, TerminalResponse, WorkspaceId,
};

const TERMINAL_START: &str = r#"{"request_id":"request-5","type":"terminal","payload":{"operation":"start","workspace_id":"workspace-1","columns":80,"rows":24}}"#;
const TERMINAL_STARTED: &str = r#"{"request_id":"request-5","type":"terminal","payload":{"result":"started","id":"terminal-1"}}"#;

#[test]
fn terminal_wire_round_trips_a_workspace_scoped_request() {
    let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
    let request = RequestEnvelope {
        request_id: RequestId::try_new("request-5").unwrap(),
        body: RequestBody::Terminal(TerminalRequest::Start {
            workspace_id,
            cwd: None,
            columns: 80,
            rows: 24,
        }),
    };
    assert_eq!(serde_json::to_string(&request).unwrap(), TERMINAL_START);
    let decoded: RequestEnvelope = serde_json::from_str(TERMINAL_START).unwrap();
    assert_eq!(decoded, request);

    let response = ResponseEnvelope {
        request_id: RequestId::try_new("request-5").unwrap(),
        body: ResponseBody::Terminal(TerminalResponse::Started {
            id: TerminalId::new("terminal-1"),
        }),
    };
    assert_eq!(serde_json::to_string(&response).unwrap(), TERMINAL_STARTED);
    assert!(response.matches_request(&decoded));
}
