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

use client_api::{AuthRequired, ClientMessage, HelloRequest, ServerMessage, WorkspaceId};
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
    pub hello: HelloRequest,
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
    discover_inner(issuer, false)
}

#[cfg(test)]
fn discover_http(issuer: &str) -> Result<Discovery, Error> {
    discover_inner(issuer, true)
}

fn discover_inner(issuer: &str, allow_http: bool) -> Result<Discovery, Error> {
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
    validate_endpoint(
        "authorization_endpoint",
        &discovery.authorization_endpoint,
        allow_http,
    )?;
    validate_endpoint("token_endpoint", &discovery.token_endpoint, allow_http)?;
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

fn validate_endpoint(name: &'static str, value: &str, allow_http: bool) -> Result<(), Error> {
    if value.chars().any(char::is_control) {
        return Err(format!("OIDC {name} が不正です").into());
    }
    let url = reqwest::Url::parse(value)?;
    let scheme_allowed = url.scheme() == "https" || (allow_http && url.scheme() == "http");
    if !scheme_allowed || url.host_str().is_none() || url.fragment().is_some() {
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
                // listener の non-blocking 設定を accept 後に引き継がない。
                stream.set_nonblocking(false)?;
                let remaining = deadline.saturating_duration_since(Instant::now());
                stream.set_read_timeout(Some(remaining.min(Duration::from_secs(5))))?;
                let target = match read_request_target(&stream, deadline) {
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

fn read_request_target(stream: &TcpStream, deadline: Instant) -> std::io::Result<String> {
    let mut reader = stream.try_clone()?;
    let head = read_http_head(&mut reader, 8192, deadline)?;
    let request_line = head.split(|byte| *byte == b'\n').next().unwrap_or_default();
    let line = std::str::from_utf8(request_line)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid request"))?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing target"))?
        .to_owned();
    if method != "GET" {
        return Ok("/invalid-method".to_owned());
    }
    Ok(target)
}

/// HTTP のヘッダー終端まで読み切る。読み取りは絶対 deadline で打ち切る。
fn read_http_head(
    reader: &mut impl Read,
    max: usize,
    deadline: Instant,
) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "callback timed out",
            ));
        }
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
        if head.windows(4).any(|window| window == b"\r\n\r\n")
            || head.windows(2).any(|window| window == b"\n\n")
        {
            break;
        }
        if head.len() >= max {
            break;
        }
    }
    Ok(head)
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
    fn callback_accepts_request_sent_after_the_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            // accept 直後はまだ要求が無い状態を作る。
            std::thread::sleep(Duration::from_millis(250));
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
            wait_for_callback(listener, "/auth/callback", "s1", Duration::from_secs(5)).unwrap();
        assert_eq!(code, "abc");
        client.join().unwrap();
    }

    #[test]
    fn slow_request_body_does_not_exceed_the_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            // 改行を送らずに接続を占有し続ける。
            std::thread::sleep(Duration::from_millis(600));
            let _ = stream.write_all(b"GET /auth/callback?code=abc");
        });
        let started = Instant::now();
        let result =
            wait_for_callback(listener, "/auth/callback", "s1", Duration::from_millis(300));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        client.join().unwrap();
    }

    #[test]
    fn endpoints_must_use_https() {
        assert!(
            validate_endpoint("authorization_endpoint", "http://idp.example/auth", false).is_err()
        );
        assert!(validate_endpoint("token_endpoint", "https://idp.example/token", false).is_ok());
    }

    #[test]
    fn redirect_uri_rejects_remote_hosts() {
        assert!(parse_redirect_uri("http://192.0.2.1:53682/auth/callback").is_err());
        assert!(parse_redirect_uri("http://localhost:53682/auth/callback").is_err());
        assert!(parse_redirect_uri("http://127.0.0.1:53682/auth/callback").is_ok());
    }

    /// 実 IdP の代わりに discovery / JWKS / token を返す最小の provider。
    struct MockProvider {
        base: String,
    }

    impl MockProvider {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let token = crate::oidc::test_token_for(
                &base,
                "alice",
                jsonwebtoken::get_current_timestamp() + 300,
            );
            let server_base = base.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else {
                        break;
                    };
                    let Some(request) = read_mock_request(&mut stream) else {
                        continue;
                    };
                    let (status, body) = match (request.method.as_str(), request.path.as_str()) {
                        ("GET", "/.well-known/openid-configuration") => (
                            "200 OK",
                            serde_json::json!({
                                "issuer": server_base,
                                "authorization_endpoint": format!("{server_base}/authorize"),
                                "token_endpoint": format!("{server_base}/token"),
                                "jwks_uri": format!("{server_base}/jwks"),
                            })
                            .to_string(),
                        ),
                        ("GET", "/jwks") => ("200 OK", crate::oidc::TEST_JWKS.to_owned()),
                        ("POST", "/token")
                            if [
                                "grant_type=authorization_code",
                                "code=code-1",
                                "redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2Fauth%2Fcallback",
                                "client_id=lapis-desktop",
                                "code_verifier=verifier-1",
                            ]
                            .iter()
                            .all(|field| request.body.contains(field)) =>
                        {
                            (
                                "200 OK",
                                serde_json::json!({ "id_token": token }).to_string(),
                            )
                        }
                        ("POST", "/token") => ("400 Bad Request", "{}".to_owned()),
                        _ => ("404 Not Found", "{}".to_owned()),
                    };
                    let _ = write_mock_response(&mut stream, status, &body);
                }
            });
            Self { base }
        }
    }

    struct MockRequest {
        method: String,
        path: String,
        body: String,
    }

    fn read_mock_request(stream: &mut TcpStream) -> Option<MockRequest> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        let head_end = loop {
            let read = stream.read(&mut chunk).ok()?;
            if read == 0 {
                return None;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            if buffer.len() > 64 * 1024 {
                return None;
            }
        };
        let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next()?.split_whitespace();
        let method = request_line.next()?.to_owned();
        let path = request_line.next()?.split('?').next()?.to_owned();
        let content_length = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = buffer[head_end..].to_vec();
        while body.len() < content_length {
            let read = stream.read(&mut chunk).ok()?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
        Some(MockRequest {
            method,
            path,
            body: String::from_utf8_lossy(&body[..body.len().min(content_length)]).to_string(),
        })
    }

    fn write_mock_response(
        stream: &mut TcpStream,
        status: &str,
        body: &str,
    ) -> std::io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        stream.flush()
    }

    #[test]
    fn mock_provider_token_is_accepted_after_discovery_and_exchange() {
        let provider = MockProvider::start();
        let discovery = discover_http(&provider.base).unwrap();
        assert_eq!(
            discovery.authorization_endpoint,
            format!("{}/authorize", provider.base)
        );
        assert_eq!(discovery.token_endpoint, format!("{}/token", provider.base));

        let redirect_uri = "http://127.0.0.1:53682/auth/callback";
        let token = exchange(
            &discovery.token_endpoint,
            "lapis-desktop",
            "code-1",
            "verifier-1",
            redirect_uri,
        )
        .unwrap();

        let config = crate::oidc::OidcConfig::new(
            provider.base.clone(),
            "lapis",
            vec![jsonwebtoken::Algorithm::RS256],
            60,
        )
        .unwrap();
        let verifier =
            crate::oidc::OidcVerifier::from_remote_http(config, format!("{}/jwks", provider.base))
                .unwrap();
        let claims = verifier.verify(token.as_str()).unwrap();
        assert_eq!(claims.subject, "alice");
        assert_eq!(claims.issuer, provider.base);
    }
}
