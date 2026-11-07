//! Desktop 向け OIDC Authorization Code + PKCE ログイン。
//!
//! Host の `AuthRequired.oidc` から IdP 設定を取得し、システムブラウザーと
//! loopback callback で ID Token を受け取る。Token は呼出側が保護保存する。

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    time::{Duration, Instant},
};

use client_api::{AuthRequired, ClientHello, ClientMessage, ServerMessage, WorkspaceId};
use client_core::pkce::{Pkce, authorization_url};
use rustls::pki_types::CertificateDer;
use serde::Deserialize;
use zeroize::Zeroizing;

type Error = Box<dyn std::error::Error + Send + Sync>;

const MAX_DISCOVERY: usize = 64 * 1024;
const MAX_TOKEN_RESPONSE: usize = 1024 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);

const SUCCESS_HTML: &str = "<!doctype html><meta charset=\"utf-8\"><title>Lapis</title><p>ログインが完了しました。Lapis に戻ってください。</p>";
const ERROR_HTML: &str = "<!doctype html><meta charset=\"utf-8\"><title>Lapis</title><p>ログインできませんでした。Lapis に戻ってやり直してください。</p>";

/// Desktop の OIDC ログイン入力。
pub struct DesktopOidcLogin {
    pub address: std::net::SocketAddr,
    pub server_name: String,
    pub certificate: CertificateDer<'static>,
    pub hello: ClientHello,
    pub workspace_id: WorkspaceId,
    /// IdP に登録済みの loopback redirect URI。
    pub redirect_uri: String,
}

/// 検証済みの ID Token とアカウント。
pub struct DesktopOidcSession {
    pub id_token: Zeroizing<String>,
    pub subject: String,
}

/// OIDC ログインを実行し、Host が検証した ID Token と `sub` を返す。
pub fn login_desktop_oidc(request: DesktopOidcLogin) -> Result<DesktopOidcSession, Error> {
    let redirect_uri = parse_redirect_uri(&request.redirect_uri)?;
    let listener = bind_callback(&redirect_uri)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let required = read_auth_required(&runtime, &request)?;
    let oidc = required
        .oidc
        .ok_or("Host が OIDC ログインを提示していません")?;

    let discovery = discover(&oidc.issuer)?;
    let mut verifier_entropy = [0u8; 32];
    let mut state_entropy = [0u8; 16];
    getrandom::fill(&mut verifier_entropy)?;
    getrandom::fill(&mut state_entropy)?;
    let pkce = Pkce::from_entropy(&verifier_entropy, &state_entropy)
        .map_err(|_| "PKCE を生成できません")?;
    let scopes = oidc.scopes.iter().map(String::as_str).collect::<Vec<_>>();
    let url = authorization_url(
        &discovery.authorization_endpoint,
        &oidc.client_id,
        redirect_uri.as_str(),
        &scopes,
        &pkce,
    )
    .map_err(|_| "Authorization URL を生成できません")?;

    open_browser(&url)?;
    let code = wait_for_callback(listener, redirect_uri.path(), pkce.state(), LOGIN_TIMEOUT)?;
    let id_token = exchange(
        &discovery.token_endpoint,
        &oidc.client_id,
        &code,
        pkce.verifier(),
        redirect_uri.as_str(),
    )?;

    let config = crate::QuicConnectionConfig::oidc(
        request.address,
        request.server_name,
        request.certificate,
        request.hello,
        request.workspace_id,
        id_token.to_string(),
    );
    let hello = runtime.block_on(config.verify())?;
    let subject = hello.subject.ok_or("Host が OIDC subject を返しません")?;
    Ok(DesktopOidcSession { id_token, subject })
}

fn read_auth_required(
    runtime: &tokio::runtime::Runtime,
    request: &DesktopOidcLogin,
) -> Result<AuthRequired, Error> {
    runtime.block_on(async {
        let mut client = crate::QuicClient::connect(
            request.address,
            &request.server_name,
            request.certificate.clone(),
            crate::RemoteLimits::default(),
        )
        .await?;
        client
            .send(&ClientMessage::Hello(request.hello.clone()))
            .await?;
        let message = tokio::time::timeout(AUTH_TIMEOUT, client.recv()).await??;
        match message {
            ServerMessage::AuthRequired(required) => Ok(required),
            _ => Err("Host が認証方式を返しません".into()),
        }
    })
}

#[derive(Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
}

fn discover(issuer: &str) -> Result<Discovery, Error> {
    let issuer = issuer.trim_end_matches('/');
    let url = format!("{issuer}/.well-known/openid-configuration");
    let client = reqwest::blocking::Client::builder()
        .timeout(AUTH_TIMEOUT)
        .redirect(https_redirects())
        .build()?;
    let response = client.get(&url).send()?;
    if !response.status().is_success() {
        return Err("OIDC discovery が失敗しました".into());
    }
    let mut body = Vec::new();
    response
        .take(MAX_DISCOVERY as u64 + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_DISCOVERY {
        return Err("OIDC discovery の応答が大きすぎます".into());
    }
    let discovery: Discovery = serde_json::from_slice(&body)?;
    validate_endpoint("authorization_endpoint", &discovery.authorization_endpoint)?;
    validate_endpoint("token_endpoint", &discovery.token_endpoint)?;
    Ok(discovery)
}

fn https_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.url().scheme() != "https" {
            attempt.error("HTTPS redirect required")
        } else {
            attempt.follow()
        }
    })
}

fn validate_endpoint(name: &'static str, value: &str) -> Result<(), Error> {
    if value.chars().any(char::is_control) {
        return Err(format!("OIDC {name} が不正です").into());
    }
    let url = reqwest::Url::parse(value)?;
    if url.scheme() != "https" || url.host_str().is_none() || url.fragment().is_some() {
        return Err(format!("OIDC {name} は HTTPS URL で指定してください").into());
    }
    Ok(())
}

fn parse_redirect_uri(value: &str) -> Result<reqwest::Url, Error> {
    let url = reqwest::Url::parse(value)?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1"))
        || url.port().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("OIDC redirect URI は http://127.0.0.1:<port>/... で指定してください".into());
    }
    Ok(url)
}

fn bind_callback(redirect_uri: &reqwest::Url) -> Result<TcpListener, Error> {
    let port = redirect_uri
        .port()
        .ok_or("OIDC redirect URI に port がありません")?;
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Code(String),
    Error(String),
    Ignore,
}

fn wait_for_callback(
    listener: TcpListener,
    path: &str,
    state: &str,
    timeout: Duration,
) -> Result<String, Error> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let target = match read_request_target(&stream) {
                    Ok(target) => target,
                    Err(_) => {
                        let _ = respond(&mut stream, "400 Bad Request", ERROR_HTML);
                        continue;
                    }
                };
                match parse_callback(&target, path, state) {
                    Callback::Code(code) => {
                        respond(&mut stream, "200 OK", SUCCESS_HTML)?;
                        return Ok(code);
                    }
                    Callback::Error(error) => {
                        let _ = respond(&mut stream, "400 Bad Request", ERROR_HTML);
                        return Err(error.into());
                    }
                    Callback::Ignore => {
                        let _ = respond(&mut stream, "400 Bad Request", ERROR_HTML);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("OIDC ログインがtimeoutしました".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn read_request_target(stream: &TcpStream) -> std::io::Result<String> {
    let mut reader = stream.try_clone()?;
    let request_line = read_until_newline(&mut reader, 8192)?;
    let line = std::str::from_utf8(&request_line)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid request"))?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing target"))?
        .to_owned();
    // 応答前にヘッダーを読み切り、未読データによる RST を避ける。
    let mut headers = 0;
    loop {
        let line = read_until_newline(&mut reader, 8192)?;
        if line.is_empty() || line == b"\r\n" || line == b"\n" {
            break;
        }
        headers += line.len();
        if headers >= 8192 {
            break;
        }
    }
    if method != "GET" {
        return Ok("/invalid-method".to_owned());
    }
    Ok(target)
}

fn read_until_newline(reader: &mut impl Read, max: usize) -> std::io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if reader.read(&mut byte)? == 0 {
            break;
        }
        line.push(byte[0]);
        if byte[0] == b'\n' || line.len() >= max {
            break;
        }
    }
    Ok(line)
}

fn parse_callback(target: &str, expected_path: &str, expected_state: &str) -> Callback {
    if !target.starts_with('/') {
        return Callback::Ignore;
    }
    let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Ignore;
    };
    if url.path() != expected_path {
        return Callback::Ignore;
    }
    let (mut code, mut state, mut error) = (None, None, None);
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Callback::Ignore;
    }
    if let Some(error) = error {
        return Callback::Error(format!("IdP がログインを拒否しました: {error}"));
    }
    match code.filter(|value| !value.is_empty()) {
        Some(code) => Callback::Code(code),
        None => Callback::Error("OIDC callback に code がありません".to_owned()),
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    error: Option<String>,
}

fn exchange(
    token_endpoint: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Zeroizing<String>, Error> {
    let client = reqwest::blocking::Client::builder()
        .timeout(EXCHANGE_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    let response = client.post(token_endpoint).form(&form).send()?;
    if !response.status().is_success() {
        return Err(format!("OIDC token endpoint が {} を返しました", response.status()).into());
    }
    let mut body = Vec::new();
    response
        .take(MAX_TOKEN_RESPONSE as u64 + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_TOKEN_RESPONSE {
        return Err("OIDC token 応答が大きすぎます".into());
    }
    let token: TokenResponse = serde_json::from_slice(&body)?;
    if token.error.is_some() {
        return Err("OIDC token endpoint が code を拒否しました".into());
    }
    let id_token = token
        .id_token
        .filter(|value| !value.is_empty())
        .ok_or("OIDC token 応答に ID Token がありません")?;
    Ok(Zeroizing::new(id_token))
}

fn open_browser(url: &str) -> Result<(), Error> {
    #[cfg(target_os = "windows")]
    let status = Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .status()?;
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(url).status()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(url).status()?;
    if !status.success() {
        return Err("ブラウザーを開けませんでした".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_requires_matching_path_and_state() {
        assert_eq!(
            parse_callback("/auth/callback?code=abc&state=s1", "/auth/callback", "s1"),
            Callback::Code("abc".to_owned())
        );
        assert_eq!(
            parse_callback(
                "/auth/callback?code=abc&state=wrong",
                "/auth/callback",
                "s1"
            ),
            Callback::Ignore
        );
        assert_eq!(
            parse_callback("/other?code=abc&state=s1", "/auth/callback", "s1"),
            Callback::Ignore
        );
    }

    #[test]
    fn callback_reports_idp_error_and_missing_code() {
        assert_eq!(
            parse_callback(
                "/auth/callback?error=access_denied&state=s1",
                "/auth/callback",
                "s1"
            ),
            Callback::Error("IdP がログインを拒否しました: access_denied".to_owned())
        );
        assert_eq!(
            parse_callback("/auth/callback?state=s1", "/auth/callback", "s1"),
            Callback::Error("OIDC callback に code がありません".to_owned())
        );
    }

    #[test]
    fn callback_listener_accepts_matching_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .write_all(
                    b"GET /auth/callback?code=abc&state=s1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                )
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 200"));
        });
        let code =
            wait_for_callback(listener, "/auth/callback", "s1", Duration::from_secs(2)).unwrap();
        assert_eq!(code, "abc");
        client.join().unwrap();
    }

    #[test]
    fn endpoints_must_use_https() {
        assert!(validate_endpoint("authorization_endpoint", "http://idp.example/auth").is_err());
        assert!(validate_endpoint("token_endpoint", "https://idp.example/token").is_ok());
    }

    #[test]
    fn redirect_uri_rejects_remote_hosts() {
        assert!(parse_redirect_uri("http://192.0.2.1:53682/auth/callback").is_err());
        assert!(parse_redirect_uri("http://localhost:53682/auth/callback").is_err());
        assert!(parse_redirect_uri("http://127.0.0.1:53682/auth/callback").is_ok());
    }
}
