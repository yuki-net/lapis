//! Terminal 拡張が公開するセッション操作の DTO。

pub mod v1 {
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    pub const START: &str = "terminal.v1.start";
    pub const INFO: &str = "terminal.v1.info";
    pub const LIST: &str = "terminal.v1.list";
    pub const WRITE: &str = "terminal.v1.write";
    pub const RESIZE: &str = "terminal.v1.resize";
    pub const READ: &str = "terminal.v1.read";
    pub const CLOSE: &str = "terminal.v1.close";
    pub const MAX_READ_BYTES: u32 = 8 * 1024;
    pub const MAX_READ_WAIT_MS: u32 = 1_000;

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ListRequest {}

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct InfoRequest {}

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PtyFlavor {
        Conpty,
        Posix,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ShellCommand {
        pub program: String,
        pub arguments: Vec<String>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct InfoResponse {
        pub pty: PtyFlavor,
        pub default_shell: ShellCommand,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct StartRequest {
        pub program: String,
        #[serde(default)]
        pub arguments: Vec<String>,
        #[serde(default)]
        pub environment: BTreeMap<String, String>,
        pub working_directory: String,
        pub columns: u16,
        pub rows: u16,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct IdRequest {
        pub id: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct WriteRequest {
        pub id: String,
        pub bytes: Vec<u8>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ResizeRequest {
        pub id: String,
        pub columns: u16,
        pub rows: u16,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ReadRequest {
        pub id: String,
        pub offset: u64,
        pub max_bytes: u32,
        pub timeout_ms: u32,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Session {
        pub id: String,
        pub columns: u16,
        pub rows: u16,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ListResponse {
        pub sessions: Vec<Session>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct StartResponse {
        pub id: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Exit {
        pub code: Option<i32>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ReadResponse {
        pub bytes: Vec<u8>,
        pub next: u64,
        pub lost_before: Option<u64>,
        pub exit: Option<Exit>,
        pub read_error: Option<String>,
        pub write_error: Option<String>,
    }
}

#[cfg(test)]
mod tests {
    use super::v1::*;

    #[test]
    fn request_defaults_and_unknown_fields_are_checked() {
        let request: StartRequest = serde_json::from_str(
            r#"{"program":"shell","working_directory":".","columns":80,"rows":24}"#,
        )
        .unwrap();
        assert!(request.arguments.is_empty());
        assert!(request.environment.is_empty());
        assert!(serde_json::from_str::<ListRequest>(r#"{"unknown":true}"#).is_err());
    }

    #[test]
    fn exit_without_code_is_distinct_from_a_running_session() {
        let output = ReadResponse {
            bytes: vec![],
            next: 0,
            lost_before: None,
            exit: Some(Exit { code: None }),
            read_error: None,
            write_error: None,
        };
        let value = serde_json::to_value(&output).unwrap();
        assert_eq!(value["exit"], serde_json::json!({"code": null}));
        assert_eq!(
            serde_json::from_value::<ReadResponse>(value).unwrap(),
            output
        );
    }
}
