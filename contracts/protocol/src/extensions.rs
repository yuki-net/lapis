use serde::{Deserialize, Serialize};

/// Host が持つ拡張の目録を要求する。
///
/// 拡張は Host 全体の資源であり Workspace に属さないため、`workspace_id` を持たない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionsListRequest {}

/// 拡張の次回起動時の有効状態を変更する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionSetEnabledRequest {
    pub id: String,
    pub enabled: bool,
}

/// 要求を目録へ保存したことを示す。実行中の状態は変更しない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionSetEnabledResponse {
    pub id: String,
    pub enabled: bool,
}

/// 検証済み package が要求する能力と承認状態を取得する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionCapabilitiesRequest {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionCapability {
    pub id: String,
    pub version: String,
    pub approved: bool,
    pub supported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionCapabilitiesResponse {
    pub id: String,
    /// 承認対象の検証済み manifest。更新後は再確認を要求する。
    pub manifest_digest: String,
    pub capabilities: Vec<HostExtensionCapability>,
}

/// ユーザーが指定した能力の承認だけを変更する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionSetCapabilityRequest {
    pub id: String,
    pub manifest_digest: String,
    pub capability: String,
    pub version: String,
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionSetCapabilityResponse {
    pub id: String,
    pub manifest_digest: String,
    pub capability: String,
    pub version: String,
    pub approved: bool,
}

/// Host 全体の拡張操作権限で、起動時に検証済みの拡張に操作を送る。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionDispatchRequest {
    pub id: String,
    pub operation: String,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionDispatchResponse {
    pub id: String,
    pub operation: String,
    pub result: String,
}

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
    /// 現在の稼働状態。旧 Host や起動中は不明。
    #[serde(default)]
    pub active: Option<bool>,
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
    /// 起動時に検証され、現在利用できる拡張の静的宣言。
    #[serde(default)]
    pub contributions: Vec<HostExtensionContributions>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExtensionContributions {
    /// Host が署名済み manifest から付与した提供元 ID。
    pub extension_id: String,
    pub declaration: extension_ui_contract::StaticContributions,
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
    fn changing_enabled_state_requires_host_management() {
        let request = RequestBody::HostExtensionSetEnabled(HostExtensionSetEnabledRequest {
            id: "dev.example.terminal".to_owned(),
            enabled: false,
        });
        assert_eq!(request.method(), "host.extensions.set_enabled");
        assert_eq!(
            request.required_capability(),
            crate::capability::HOST_MANAGE
        );
        assert_eq!(request.workspace_id(), None);
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serde_json::from_value::<RequestBody>(encoded).unwrap(),
            request
        );
    }

    #[test]
    fn capability_inspection_and_changes_have_separate_permissions() {
        let read = RequestBody::HostExtensionCapabilities(HostExtensionCapabilitiesRequest {
            id: "dev.example".into(),
        });
        let write = RequestBody::HostExtensionSetCapability(HostExtensionSetCapabilityRequest {
            id: "dev.example".into(),
            manifest_digest: "a".repeat(64),
            capability: "process.pty".into(),
            version: "1.0.0".into(),
            approved: true,
        });
        assert_eq!(
            read.required_capability(),
            crate::capability::EXTENSIONS_READ
        );
        assert_eq!(
            write.required_capability(),
            crate::capability::EXTENSIONS_MANAGE
        );
        assert_eq!(read.workspace_id(), None);
        assert_eq!(write.workspace_id(), None);
        assert_eq!(
            serde_json::from_str::<RequestBody>(&serde_json::to_string(&write).unwrap()).unwrap(),
            write
        );
    }

    #[test]
    fn extension_dispatch_requires_explicit_host_wide_capability() {
        let request = RequestBody::HostExtensionDispatch(HostExtensionDispatchRequest {
            id: "dev.example".into(),
            operation: "inspect".into(),
            payload: "{}".into(),
        });
        assert_eq!(
            request.required_capability(),
            crate::capability::EXTENSIONS_EXECUTE
        );
        assert_eq!(request.workspace_id(), None);
        assert_eq!(
            serde_json::from_value::<RequestBody>(serde_json::to_value(&request).unwrap()).unwrap(),
            request
        );
    }

    #[test]
    fn declarations_round_trip_through_the_common_envelopes() {
        let response = HostExtensionsListResponse {
            extensions: vec![HostExtension {
                id: "dev.example.terminal".to_owned(),
                version: "0.2.0".to_owned(),
                installed: true,
                enabled: false,
                active: None,
                incompatible: false,
            }],
            contributions: vec![HostExtensionContributions {
                extension_id: "dev.example.terminal".to_owned(),
                declaration: extension_ui_contract::StaticContributions {
                    schema: 1,
                    default_locale: "en-US".to_owned(),
                    settings: Vec::new(),
                    themes: Vec::new(),
                    translations: vec![extension_ui_contract::ContributedTranslation {
                        target: "dev.example.terminal".to_owned(),
                        locale: "ja-JP".to_owned(),
                        messages: [("title".to_owned(), "端末".to_owned())]
                            .into_iter()
                            .collect(),
                    }],
                },
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
        let old_response: HostExtensionsListResponse =
            serde_json::from_str(r#"{"extensions":[]}"#).unwrap();
        assert!(old_response.contributions.is_empty());
    }
}
