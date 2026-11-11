//! Rust を正本とした wire fixture。
//!
//! Mobile の `ProtocolSerializationTest.kt` と同じ JSON を検証し、
//! Kotlin / Rust 間の DTO 差分を検出する。

use std::path::Path;

use client_api::{
    AuthMethod, AuthRequired, AuthenticateRequest, CURRENT_PROTOCOL, ClientId, ClientKind,
    ClientMessage, CredentialId, ErrorCode, EventBody, EventEnvelope, EventSequence, HelloRequest,
    HelloResponse, INVALID_REQUEST, OidcClientConfig, OidcRequest, PairedResponse, ProtocolError,
    ProtocolRange, RequestBody, RequestEnvelope, RequestId, ServerMessage, SessionId, WorkspaceId,
    WorkspaceListRequest,
};

const CLIENT_HELLO: &str = r#"{"protocol":{"major":0,"minimum_minor":11,"maximum_minor":11},"client_id":"mobile-1","client_name":"Mobile","client_kind":"android","requested_capabilities":[]}"#;
const AUTHENTICATE: &str = r#"{"type":"auth.authenticate","payload":{"hello":{"protocol":{"major":0,"minimum_minor":11,"maximum_minor":11},"client_id":"mobile-1","client_name":"Mobile","client_kind":"android","requested_capabilities":[]},"workspace_id":"workspace-1","credential_id":"0123456789abcdef0123456789abcdef","secret":"abababababababababababababababababababababababababababababababab"}}"#;
const OIDC: &str = r#"{"type":"auth.oidc","payload":{"hello":{"protocol":{"major":0,"minimum_minor":11,"maximum_minor":11},"client_id":"mobile-1","client_name":"Mobile","client_kind":"android","requested_capabilities":[]},"workspace_id":"workspace-1","id_token":"test-token"}}"#;
const AUTH_REQUIRED: &str = r#"{"type":"auth.required","payload":{"methods":["token","oidc"],"oidc":{"issuer":"https://idp.example","client_id":"lapis","scopes":["openid","profile"]}}}"#;
const REQUEST_ENVELOPE: &str = r#"{"request_id":"request-1","type":"workspace.list","payload":{}}"#;
const EVENT_ENVELOPE: &str =
    r#"{"event_sequence":7,"type":"workspace.changed","payload":{"workspace_id":"workspace-1"}}"#;
const PROTOCOL_ERROR: &str =
    r#"{"type":"error","payload":{"code":"invalid_request","retryable":false}}"#;
const PAIRED_RESPONSE: &str = r#"{"type":"auth.paired","payload":{"hello":{"protocol":{"major":0,"minor":11},"session_id":"session-1","granted_capabilities":[]},"credential_id":"0123456789abcdef0123456789abcdef","secret":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"}}"#;

const MOBILE_TEST_PATH: &str = "../../clients/mobile/sharedUi/src/commonTest/kotlin/dev/lapis/mobile/protocol/ProtocolSerializationTest.kt";

fn hello() -> HelloRequest {
    HelloRequest {
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
fn oidc_message_matches_mobile_fixture() {
    let request = OidcRequest::new(
        hello(),
        WorkspaceId::try_new("workspace-1").unwrap(),
        "test-token".to_owned(),
    );
    let json = serde_json::to_string(&ClientMessage::Oidc(request)).unwrap();
    assert_eq!(json, OIDC);
}

#[test]
fn auth_required_matches_mobile_fixture() {
    let message = ServerMessage::AuthRequired(AuthRequired {
        methods: vec![AuthMethod::Token, AuthMethod::Oidc],
        oidc: Some(OidcClientConfig {
            issuer: "https://idp.example".to_owned(),
            client_id: "lapis".to_owned(),
            scopes: vec!["openid".to_owned(), "profile".to_owned()],
        }),
    });
    let json = serde_json::to_string(&message).unwrap();
    assert_eq!(json, AUTH_REQUIRED);
    let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), AUTH_REQUIRED);
}

#[test]
fn auth_required_without_oidc_is_backward_compatible() {
    let message: ServerMessage =
        serde_json::from_str(r#"{"type":"auth.required","payload":{"methods":["token"]}}"#)
            .unwrap();
    let ServerMessage::AuthRequired(required) = message else {
        panic!("expected auth.required");
    };
    assert_eq!(required.methods, vec![AuthMethod::Token]);
    assert_eq!(required.oidc, None);
    assert_eq!(
        serde_json::to_string(&ServerMessage::AuthRequired(required)).unwrap(),
        r#"{"type":"auth.required","payload":{"methods":["token"]}}"#
    );
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
    let server_hello = HelloResponse {
        protocol: CURRENT_PROTOCOL,
        session_id: SessionId::try_new("session-1").unwrap(),
        granted_capabilities: Default::default(),
        subject: None,
        host_id: None,
        workspace_id: None,
    };
    let response = PairedResponse::new(server_hello, &credential, &[0xcd; 32]);
    let json = serde_json::to_string(&ServerMessage::Paired(response)).unwrap();
    assert_eq!(json, PAIRED_RESPONSE);
}

#[test]
fn server_hello_subject_is_optional() {
    let subject = HelloResponse {
        protocol: CURRENT_PROTOCOL,
        session_id: SessionId::try_new("session-1").unwrap(),
        granted_capabilities: Default::default(),
        subject: Some("alice".to_owned()),
        host_id: None,
        workspace_id: None,
    };
    let json = serde_json::to_string(&subject).unwrap();
    assert!(json.contains("\"subject\":\"alice\""));
    let without = HelloResponse {
        subject: None,
        ..subject
    };
    assert!(!serde_json::to_string(&without).unwrap().contains("subject"));
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
        OIDC,
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
