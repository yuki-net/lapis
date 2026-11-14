use serde::{Deserialize, Serialize};

/// Host が持つ拡張の目録を要求する。
///
/// 拡張は Host 全体の資源であり Workspace に属さないため、`workspace_id` を持たない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionsListRequest {}

/// 導入済み拡張 1 件の宣言。`enabled` は `installed` を含意する。
///
/// 状態は Host の正本をそのまま写した値であり、Client はこれを表示にだけ使い、
/// 保存済みの値を正本として扱わない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtension {
    /// package が宣言する ID。Host のデータ領域では directory 名になる。
    pub id: String,
    pub version: String,
    pub installed: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionsListResponse {
    pub extensions: Vec<HostExtension>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RequestBody, ResponseBody};

    #[test]
    fn the_list_is_a_host_level_read_without_a_workspace() {
        let request = RequestBody::HostExtensions(HostExtensionsListRequest {});
        assert_eq!(request.method(), "host.extensions.list");
        assert_eq!(request.workspace_id(), None);
    }

    #[test]
    fn declarations_round_trip_through_the_common_envelopes() {
        let response = HostExtensionsListResponse {
            extensions: vec![HostExtension {
                id: "dev.example.terminal".to_owned(),
                version: "0.2.0".to_owned(),
                installed: true,
                enabled: false,
            }],
        };
        let body = ResponseBody::HostExtensions(response.clone());
        assert_eq!(body.method(), Some("host.extensions.list"));
        let encoded = serde_json::to_value(&body).unwrap();
        assert_eq!(encoded["type"], "host.extensions.list");
        assert_eq!(
            serde_json::from_value::<ResponseBody>(encoded).unwrap(),
            body
        );
    }
}
