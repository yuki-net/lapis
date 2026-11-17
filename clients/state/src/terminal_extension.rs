//! Terminal 拡張の版付き DTO と汎用拡張 transport の接着部。

use client_api::{HostExtensionDispatchRequest, HostExtensionDispatchResponse};
use serde::{Serialize, de::DeserializeOwned};

pub use terminal_session_api::v1;

pub const EXTENSION_ID: &str = "lapis.terminal";

/// 呼び出し元は v1 の操作名と対応する要求 DTO を指定する。
pub fn request<T: Serialize>(
    operation: &str,
    value: &T,
) -> Result<HostExtensionDispatchRequest, String> {
    Ok(HostExtensionDispatchRequest {
        id: EXTENSION_ID.to_owned(),
        operation: operation.to_owned(),
        payload: serde_json::to_string(value).map_err(|error| error.to_string())?,
    })
}

pub fn response<T: DeserializeOwned>(
    operation: &str,
    response: HostExtensionDispatchResponse,
) -> Result<T, String> {
    if response.id != EXTENSION_ID || response.operation != operation {
        return Err("Terminal 拡張の応答元が一致しません".into());
    }
    serde_json::from_str(&response.result).map_err(|_| "Terminal 拡張の応答が不正です".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_wire_codec_checks_identity_operation_and_body() {
        let request = request(v1::LIST, &v1::ListRequest {}).unwrap();
        assert_eq!(request.id, EXTENSION_ID);
        assert_eq!(request.operation, v1::LIST);
        assert_eq!(request.payload, "{}");
        let reply = HostExtensionDispatchResponse {
            id: request.id,
            operation: request.operation,
            result: r#"{"sessions":[]}"#.into(),
        };
        assert_eq!(
            response::<v1::ListResponse>(v1::LIST, reply.clone())
                .unwrap()
                .sessions
                .len(),
            0
        );
        assert!(response::<v1::ListResponse>(v1::READ, reply.clone()).is_err());
        let mut wrong = reply.clone();
        wrong.id = "other".into();
        assert!(response::<v1::ListResponse>(v1::LIST, wrong).is_err());
        let mut wrong = reply;
        wrong.result = "invalid json".into();
        assert!(response::<v1::ListResponse>(v1::LIST, wrong).is_err());
    }
}
