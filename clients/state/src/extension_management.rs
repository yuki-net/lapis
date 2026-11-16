//! Web の拡張管理応答を、要求と接続に結び付けて検証する。

use client_api::ResponseBody;

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
}
