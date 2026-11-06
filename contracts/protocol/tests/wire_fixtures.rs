//! Rust を正本とした wire fixture。
//!
//! Mobile の `ProtocolSerializationTest.kt` と同じ JSON を検証し、
//! Kotlin / Rust 間の DTO 差分を検出する。

use std::path::Path;

use lapis_client_api::{
    AuthenticateRequest, CURRENT_PROTOCOL, ClientHello, ClientId, ClientKind, ClientMessage,
    CredentialId, ErrorCode, EventBody, EventEnvelope, EventSequence, INVALID_REQUEST,
    PairedResponse, ProtocolError, ProtocolRange, RequestBody, RequestEnvelope, RequestId,
    ServerHello, ServerMessage, SessionId, WorkspaceId, WorkspaceListRequest,
};

const CLIENT_HELLO: &str = r#"{"protocol":{"major":0,"minimum_minor":5,"maximum_minor":5},"client_id":"mobile-1","client_name":"Mobile","client_kind":"android","requested_capabilities":[]}"#;
const AUTHENTICATE: &str = r#"{"type":"auth.authenticate","payload":{"hello":{"protocol":{"major":0,"minimum_minor":5,"maximum_minor":5},"client_id":"mobile-1","client_name":"Mobile","client_kind":"android","requested_capabilities":[]},"workspace_id":"workspace-1","credential_id":"0123456789abcdef0123456789abcdef","secret":"abababababababababababababababababababababababababababababababab"}}"#;
const REQUEST_ENVELOPE: &str = r#"{"request_id":"request-1","type":"workspace.list","payload":{}}"#;
const EVENT_ENVELOPE: &str =
    r#"{"event_sequence":7,"type":"workspace.changed","payload":{"workspace_id":"workspace-1"}}"#;
const PROTOCOL_ERROR: &str =
    r#"{"type":"error","payload":{"code":"invalid_request","retryable":false}}"#;
const PAIRED_RESPONSE: &str = r#"{"type":"auth.paired","payload":{"hello":{"protocol":{"major":0,"minor":5},"session_id":"session-1","granted_capabilities":[]},"credential_id":"0123456789abcdef0123456789abcdef","secret":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}}"#;

const MOBILE_TEST_PATH: &str = "../../clients/mobile/sharedUi/src/commonTest/kotlin/dev/lapis/mobile/protocol/ProtocolSerializationTest.kt";

fn hello() -> ClientHello {
    ClientHello {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new("mobile-1").unwrap(),
        client_name: "Mobile".to_owned(),
        client_kind: ClientKind::Android,
        requested_capabilities: Default::default(),
    }
}

#[test]
fn client_hello_matches_mobile_fixture() {
    let json = serde_json::to_string(&hello()).unwrap();
    assert_eq!(json, CLIENT_HELLO);
}

#[test]
fn authenticate_message_matches_mobile_fixture() {
    let credential = CredentialId::parse("0123456789abcdef0123456789abcdef").unwrap();
    let request = AuthenticateRequest::new(
        hello(),
        WorkspaceId::try_new("workspace-1").unwrap(),
        &credential,
        &[0xab; 32],
    );
    let json = serde_json::to_string(&ClientMessage::Authenticate(request)).unwrap();
    assert_eq!(json, AUTHENTICATE);
}

#[test]
fn request_envelope_matches_mobile_fixture() {
    let request = RequestEnvelope {
        request_id: RequestId::try_new("request-1").unwrap(),
        body: RequestBody::WorkspaceList(WorkspaceListRequest::default()),
    };
    let json = serde_json::to_string(&request).unwrap();
    assert_eq!(json, REQUEST_ENVELOPE);
}

#[test]
fn event_envelope_matches_mobile_fixture() {
    let event = EventEnvelope {
        event_sequence: EventSequence::new(7),
        body: EventBody::WorkspaceChanged {
            workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    assert_eq!(json, EVENT_ENVELOPE);
}

#[test]
fn protocol_error_matches_mobile_fixture() {
    let error = ProtocolError::new(ErrorCode::try_new(INVALID_REQUEST).unwrap());
    let json = serde_json::to_string(&ServerMessage::Error(error)).unwrap();
    assert_eq!(json, PROTOCOL_ERROR);
}

#[test]
fn paired_response_matches_mobile_fixture() {
    let credential = CredentialId::parse("0123456789abcdef0123456789abcdef").unwrap();
    let server_hello = ServerHello {
        protocol: CURRENT_PROTOCOL,
        session_id: SessionId::try_new("session-1").unwrap(),
        granted_capabilities: Default::default(),
    };
    let response = PairedResponse::new(server_hello, &credential, &[0xcd; 32]);
    let json = serde_json::to_string(&ServerMessage::Paired(response)).unwrap();
    assert_eq!(json, PAIRED_RESPONSE);
}

/// Kotlin 側の fixture が Rust の正本と同期していることを確認する。
#[test]
fn mobile_fixtures_are_in_sync() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(MOBILE_TEST_PATH);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

    for fixture in [
        CLIENT_HELLO,
        AUTHENTICATE,
        REQUEST_ENVELOPE,
        EVENT_ENVELOPE,
        PROTOCOL_ERROR,
        PAIRED_RESPONSE,
    ] {
        assert!(
            source.contains(fixture),
            "mobile fixture is missing or out of sync: {fixture}"
        );
    }
}
