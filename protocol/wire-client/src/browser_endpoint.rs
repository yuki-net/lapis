//! ブラウザーが WebTransport の接続先と証明書ハッシュを取得する。
//!
//! Host の証明書は短期で更新されるため、ハッシュを招待へ固定しない。
//! ページは同じ Origin の `lapis-web` から都度取得し、再読み込みだけで追従する。

/// `lapis-web` が接続情報を返すパス。`host/transport/web` と一致させる。
pub const ENDPOINT_PATH: &str = "/__webtransport";

/// WebTransport の接続先と、信頼する証明書 DER の SHA-256。
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebTransportEndpoint {
    /// `https://<host>:<port>/lapis`
    pub url: String,
    /// 証明書 DER の SHA-256。
    pub certificate_sha256: Vec<u8>,
}

impl WebTransportEndpoint {
    /// ページと同じ Origin から接続情報を取得する。
    ///
    /// 取得先は現在のページの Origin に固定し、応答の URL は https のみ受け付ける。
    pub async fn fetch() -> Result<Self, String> {
        let window = web_sys::window().ok_or("ウィンドウを取得できません")?;
        let origin = window
            .location()
            .origin()
            .map_err(|_| "Originを取得できません")?;
        let url = format!("{origin}{ENDPOINT_PATH}");
        let response = gloo_net::http::Request::get(&url)
            .send()
            .await
            .map_err(|_| "接続情報を取得できません".to_owned())?;
        if !response.ok() {
            return Err(format!("接続情報を取得できません ({})", response.status()));
        }
        let body = response
            .text()
            .await
            .map_err(|_| "接続情報を読み取れません".to_owned())?;
        let endpoint: Self =
            serde_json::from_str(&body).map_err(|_| "接続情報の形式が不正です".to_owned())?;
        endpoint.validate()?;
        Ok(endpoint)
    }

    /// URL とハッシュの形を検証する。認証はここでは行わない。
    pub fn validate(&self) -> Result<(), String> {
        let url = self
            .url
            .parse::<url::Url>()
            .map_err(|_| "接続先のURLが不正です".to_owned())?;
        if url.scheme() != "https" {
            return Err("接続先は https である必要があります".into());
        }
        if url.host_str().is_none() {
            return Err("接続先のホストがありません".into());
        }
        if self.certificate_sha256.len() != 32 {
            return Err("証明書ハッシュの長さが不正です".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_requires_https_and_a_sha256_digest() {
        let valid = WebTransportEndpoint {
            url: "https://127.0.0.1:4787/lapis".into(),
            certificate_sha256: vec![0; 32],
        };
        assert!(valid.validate().is_ok());

        let insecure = WebTransportEndpoint {
            url: "http://127.0.0.1:4787/lapis".into(),
            certificate_sha256: vec![0; 32],
        };
        assert!(insecure.validate().is_err());

        let short = WebTransportEndpoint {
            url: "https://127.0.0.1:4787/lapis".into(),
            certificate_sha256: vec![0; 31],
        };
        assert!(short.validate().is_err());
    }
}
