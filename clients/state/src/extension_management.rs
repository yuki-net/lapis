//! Web の拡張管理応答を、要求と接続に結び付けて検証する。

use client_api::ResponseBody;

pub(crate) fn validate_capabilities_response(
    response: Result<ResponseBody, String>,
    current: bool,
    id: &str,
) -> Result<client_api::HostExtensionCapabilitiesResponse, String> {
    if !current {
        return Err("Host の接続が変更されました".into());
    }
    match response? {
        ResponseBody::HostExtensionCapabilities(value) if value.id == id => Ok(value),
        _ => Err("Host の能力応答が不正です".into()),
    }
}

pub(crate) fn validate_capability_update_response(
    response: Result<ResponseBody, String>,
    current: bool,
    request: &client_api::HostExtensionSetCapabilityRequest,
) -> Result<(), String> {
    if !current {
        return Err("Host の接続が変更されました".into());
    }
    match response? {
        ResponseBody::HostExtensionSetCapability(value)
            if value.id == request.id
                && value.manifest_digest == request.manifest_digest
                && value.capability == request.capability
                && value.version == request.version
                && value.approved == request.approved =>
        {
            Ok(())
        }
        _ => Err("Host の能力管理応答が不正です".into()),
    }
}

pub(crate) fn validate_response(
    response: Result<ResponseBody, String>,
    connection_current: bool,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    if !connection_current {
        return Err("Host の接続が変更されました".to_owned());
    }
    match response? {
        ResponseBody::HostExtensionSetEnabled(result)
            if result.id == id && result.enabled == enabled =>
        {
            Ok(())
        }
        _ => Err("Host の拡張管理応答が不正です".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client_api::HostExtensionSetEnabledResponse;

    #[test]
    fn failed_or_stale_web_responses_never_report_success() {
        assert_eq!(
            validate_response(Err("forbidden".into()), true, "dev.test", false),
            Err("forbidden".into())
        );
        let reply = || {
            Ok(ResponseBody::HostExtensionSetEnabled(
                HostExtensionSetEnabledResponse {
                    id: "dev.test".into(),
                    enabled: false,
                },
            ))
        };
        assert!(validate_response(reply(), false, "dev.test", false).is_err());
        assert!(validate_response(reply(), true, "dev.test", true).is_err());
        assert!(validate_response(reply(), true, "dev.test", false).is_ok());
    }

    #[test]
    fn capability_results_are_bound_to_request_and_connection() {
        let input = client_api::HostExtensionSetCapabilityRequest {
            id: "dev.example".into(),
            manifest_digest: "a".repeat(64),
            capability: "process.pty".into(),
            version: "1.0.0".into(),
            approved: true,
        };
        let reply = || {
            Ok(ResponseBody::HostExtensionSetCapability(
                client_api::HostExtensionSetCapabilityResponse {
                    id: input.id.clone(),
                    manifest_digest: input.manifest_digest.clone(),
                    capability: input.capability.clone(),
                    version: input.version.clone(),
                    approved: input.approved,
                },
            ))
        };
        assert!(validate_capability_update_response(reply(), false, &input).is_err());
        let mut wrong = input.clone();
        wrong.approved = false;
        assert!(validate_capability_update_response(reply(), true, &wrong).is_err());
        assert!(
            validate_capability_update_response(Err("forbidden".into()), true, &input).is_err()
        );
        assert!(validate_capability_update_response(reply(), true, &input).is_ok());
        let list = || {
            Ok(ResponseBody::HostExtensionCapabilities(
                client_api::HostExtensionCapabilitiesResponse {
                    id: input.id.clone(),
                    manifest_digest: input.manifest_digest.clone(),
                    capabilities: vec![],
                },
            ))
        };
        assert!(validate_capabilities_response(list(), false, &input.id).is_err());
        assert!(validate_capabilities_response(list(), true, "other").is_err());
        assert!(validate_capabilities_response(list(), true, &input.id).is_ok());
    }
}
