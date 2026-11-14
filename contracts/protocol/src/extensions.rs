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
    /// package は読めたが、要求された契約と版を Host が満たさない。
    ///
    /// `installed` / `enabled` とは排他であり、このとき実体は置いていない。
    /// この field を書かない旧い Host の宣言は `false` として読む。
    #[serde(default)]
    pub incompatible: bool,
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
                incompatible: false,
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

    #[test]
    fn a_declaration_without_the_incompatible_field_is_read_as_compatible() {
        // この field を書かない旧い Host の宣言も読める。
        let declaration: HostExtension = serde_json::from_str(
            r#"{"id":"dev.example.one","version":"0.1.0","installed":true,"enabled":true}"#,
        )
        .unwrap();
        assert!(!declaration.incompatible);
    }
}
