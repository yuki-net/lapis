//! ビルド済みの Web UI を配信する。`--dev` では `.rs` の変更で Wasm を作り直し、ブラウザーを再読み込みさせる。

use std::{
    convert::Infallible,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{
        Html, IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures_util::{Stream, stream};
use notify::Watcher;
use tokio::{
    net::TcpListener,
    process::Command,
    sync::{broadcast, mpsc},
    time::timeout,
};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

const DEFAULT_LISTEN: &str = "127.0.0.1:4747";
const DEFAULT_DIST: &str = "clients/web/dist";
/// Wasm UI の依存が収まるディレクトリ。ここにある `.rs` を監視する。
const WATCHED_DIRECTORIES: [&str; 6] = [
    "clients/ui",
    "clients/state",
    "clients/transport",
    "contracts",
    "extensions",
    "clients/web/src",
];
/// ページ本体。`.ts` や CSS の変更も再ビルドの対象にする。
const WATCHED_FILES: [&str; 1] = ["clients/web/index.html"];
const DEBOUNCE: Duration = Duration::from_millis(250);
const BUILD_TIMEOUT: Duration = Duration::from_secs(300);
const FAILURE_LINES: usize = 20;
const RELOAD_SCRIPT: &str =
    r#"new EventSource("/__reload").addEventListener("reload", () => location.reload());"#;

#[derive(Clone)]
struct AppState {
    dist: PathBuf,
    dev: bool,
    reload: broadcast::Sender<()>,
}

struct Options {
    listen: SocketAddr,
    dist: PathBuf,
    root: PathBuf,
    dev: bool,
}

impl Options {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut options = Self {
            listen: DEFAULT_LISTEN
                .parse()
                .map_err(|error| format!("invalid listen: {error}"))?,
            dist: PathBuf::from(DEFAULT_DIST),
            root: PathBuf::from("."),
            dev: false,
        };
        let mut values = arguments.iter();
        while let Some(argument) = values.next() {
            match argument.as_str() {
                "--dev" => options.dev = true,
                "--host" => {
                    // Vite と同じく、指定なしで全インターフェースを待ち受ける。
                    options.listen = SocketAddr::from(([0, 0, 0, 0], options.listen.port()));
                }
                "--listen" => {
                    let value = values.next().ok_or("--listen に値がありません")?;
                    options.listen = value
                        .parse()
                        .map_err(|error| format!("invalid listen: {error}"))?;
                }
                "--dist" => {
                    options.dist = PathBuf::from(values.next().ok_or("--dist に値がありません")?);
                }
                "--root" => {
                    options.root = PathBuf::from(values.next().ok_or("--root に値がありません")?);
                }
                "--help" => return Err(usage()),
                other => return Err(format!("unknown option: {other}")),
            }
        }
        Ok(options)
    }
}

fn usage() -> String {
    "usage: lapis-web [--dev] [--host] [--listen <addr>] [--dist <directory>] [--root <directory>]"
        .to_owned()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let options = match Options::parse(&arguments) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    run(options).await
}

async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let dist = absolute(&options.dist)?;
    let root = absolute(&options.root)?;
    if options.dev {
        build(&root).await?;
    }

    let (reload, _) = broadcast::channel(16);
    let state = AppState {
        dist: dist.clone(),
        dev: options.dev,
        reload: reload.clone(),
    };

    // 監視は配信の間ずっと生きている必要がある。
    let _watcher = if options.dev {
        let (changes, receiver) = mpsc::channel(64);
        let watcher = watch(&root, changes)?;
        tokio::spawn(rebuild_loop(root.clone(), receiver, reload));
        Some(watcher)
    } else {
        None
    };

    let app = Router::new()
        .route("/", get(index))
        // `/host/workspace` は SPA の状態を表すため、リロードでも index.html を返す。
        .route("/{host}/{workspace}", get(index))
        .route("/{host}/{workspace}/", get(index))
        .route("/__reload", get(reload_events))
        .route("/__reload.js", get(reload_script))
        .with_state(state)
        .fallback_service(ServeDir::new(&dist));
    let app = if options.dev {
        app.layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
    } else {
        app
    };
    let app = app.layer(SetResponseHeaderLayer::overriding(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    ));

    let listener = TcpListener::bind(options.listen).await?;
    let address = listener.local_addr()?;
    print_addresses(address);
    if options.dev {
        eprintln!("web: watching {}", WATCHED_DIRECTORIES.join(", "));
    }
    axum::serve(listener, app).await?;
    Ok(())
}

/// 接続に使える候補 URL を表示する。
///
/// loopback 以外で待ち受けている場合は、同じ PC のほかに LAN / Tailscale の
/// アドレスも出す。認証や到達制限は上位レイヤーの責務。
fn print_addresses(address: SocketAddr) {
    if address.ip().is_loopback() {
        eprintln!("open http://{address}/");
        return;
    }
    eprintln!("open http://127.0.0.1:{}/", address.port());
    for (label, ip) in interface_addresses() {
        eprintln!("{label} http://{ip}:{}/", address.port());
    }
}

/// ループバック以外の IPv4 アドレスを、表示用ラベルと一緒に返す。
fn interface_addresses() -> Vec<(&'static str, IpAddr)> {
    let networks = sysinfo::Networks::new_with_refreshed_list();
    let mut hosts: Vec<(&'static str, IpAddr)> = Vec::new();
    for (name, network) in networks.list() {
        for network in network.ip_networks() {
            let IpAddr::V4(ip) = network.addr else {
                continue;
            };
            if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() {
                continue;
            }
            let label = if is_tailscale(name, ip) {
                "tailscale"
            } else {
                "network"
            };
            if !hosts.iter().any(|(_, existing)| *existing == network.addr) {
                hosts.push((label, network.addr));
            }
        }
    }
    // LAN を先に、Tailscale を後に並べる。
    hosts.sort_by_key(|(label, ip)| (u8::from(*label == "tailscale"), *ip));
    hosts
}

/// Tailscale の CGNAT 範囲（100.64.0.0/10）か、名前が Tailscale の NIC か。
fn is_tailscale(interface: &str, ip: Ipv4Addr) -> bool {
    interface.to_ascii_lowercase().contains("tailscale")
        || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
}

async fn index(State(state): State<AppState>) -> Response {
    match tokio::fs::read_to_string(state.dist.join("index.html")).await {
        Ok(html) => Html(inject_reload(&html, state.dev)).into_response(),
        Err(error) => (
            StatusCode::NOT_FOUND,
            format!("index.html を読めません: {error}"),
        )
            .into_response(),
    }
}

/// 開発時だけ再読み込み用のスクリプトを差し込む。
fn inject_reload(html: &str, dev: bool) -> String {
    if !dev {
        return html.to_owned();
    }
    let script = r#"<script type="module" src="/__reload.js"></script>"#;
    match html.rfind("</body>") {
        Some(index) => format!("{}{}{}", &html[..index], script, &html[index..]),
        None => format!("{html}{script}"),
    }
}

async fn reload_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.reload.subscribe();
    let events = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(()) => {
                    return Some((
                        Ok(Event::default().event("reload").data("reload")),
                        receiver,
                    ));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}

async fn reload_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        RELOAD_SCRIPT,
    )
}

fn watch(root: &Path, changes: mpsc::Sender<()>) -> notify::Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result
            && (event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove())
            && event.paths.iter().any(|path| is_relevant(path))
        {
            let _ = changes.try_send(());
        }
    })?;
    for directory in WATCHED_DIRECTORIES {
        let path = root.join(directory);
        if path.is_dir() {
            watcher.watch(&path, notify::RecursiveMode::Recursive)?;
        }
    }
    for file in WATCHED_FILES {
        let path = root.join(file);
        if path.is_file() {
            watcher.watch(&path, notify::RecursiveMode::NonRecursive)?;
        }
    }
    Ok(watcher)
}

/// ビルド自身が書き出す生成物で再ビルドが連鎖しないようにする。
fn is_relevant(path: &Path) -> bool {
    !path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some("generated" | "dist" | "node_modules")
        )
    })
}

async fn rebuild_loop(
    root: PathBuf,
    mut changes: mpsc::Receiver<()>,
    reload: broadcast::Sender<()>,
) {
    while changes.recv().await.is_some() {
        // 連続保存を1回のビルドにまとめる。
        loop {
            match timeout(DEBOUNCE, changes.recv()).await {
                Ok(Some(_)) => continue,
                Ok(None) => return,
                Err(_) => break,
            }
        }
        match build(&root).await {
            Ok(()) => {
                eprintln!("web: rebuilt");
                let _ = reload.send(());
            }
            // 失敗したら古い Wasm を配り続け、次の保存で再試行する。
            Err(error) => eprintln!("web: build failed: {error}"),
        }
    }
}

async fn build(root: &Path) -> Result<(), String> {
    let started = std::time::Instant::now();
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let child = Command::new(npm)
        .args(["run", "build"])
        .current_dir(root.join("clients/web"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("npm run build を開始できません: {error}"))?;
    let output = timeout(BUILD_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "npm run build がtimeoutしました".to_owned())?
        .map_err(|error| format!("npm run build の待機に失敗しました: {error}"))?;
    if output.status.success() {
        eprintln!("web: built in {}s", started.elapsed().as_secs_f32().round());
        return Ok(());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<_> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(FAILURE_LINES)..].join("\n");
    Err(format!("exit code {:?}\n{tail}", output.status.code()))
}

fn absolute(path: &Path) -> Result<PathBuf, std::io::Error> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_port_and_dist() {
        let options = Options::parse(&[]).unwrap();
        assert_eq!(options.listen.to_string(), "127.0.0.1:4747");
        assert_eq!(options.dist, PathBuf::from("clients/web/dist"));
        assert!(!options.dev);
    }

    #[test]
    fn dev_flags_are_parsed() {
        let options = Options::parse(&[
            "--dev".to_owned(),
            "--listen".to_owned(),
            "127.0.0.1:5000".to_owned(),
            "--dist".to_owned(),
            "tmp/dist".to_owned(),
        ])
        .unwrap();
        assert!(options.dev);
        assert_eq!(options.listen.to_string(), "127.0.0.1:5000");
        assert_eq!(options.dist, PathBuf::from("tmp/dist"));
    }

    #[test]
    fn generated_artifacts_do_not_trigger_rebuilds() {
        assert!(is_relevant(Path::new("clients/ui/src/lib.rs")));
        assert!(!is_relevant(Path::new(
            "clients/web/src/generated/wasm/lapis_ui.js"
        )));
        assert!(!is_relevant(Path::new("clients/web/dist/main.js")));
        assert!(!is_relevant(Path::new(
            "clients/web/node_modules/typescript/lib/tsc.js"
        )));
    }

    #[test]
    fn reload_script_is_only_injected_while_developing() {
        let html = "<html><body><main></main></body></html>";
        assert!(inject_reload(html, true).contains("/__reload.js"));
        assert_eq!(inject_reload(html, false), html);
    }

    #[test]
    fn host_flag_binds_all_interfaces_on_the_same_port() {
        let options = Options::parse(&["--host".to_owned()]).unwrap();
        assert_eq!(options.listen.to_string(), "0.0.0.0:4747");
    }

    #[test]
    fn tailscale_addresses_are_labeled_separately() {
        assert!(is_tailscale("Ethernet", Ipv4Addr::new(100, 64, 1, 2)));
        assert!(is_tailscale("Tailscale", Ipv4Addr::new(192, 168, 1, 2)));
        assert!(!is_tailscale("Ethernet", Ipv4Addr::new(192, 168, 1, 2)));
        // 100.128.0.0/10 は CGNAT の範囲外。
        assert!(!is_tailscale("Ethernet", Ipv4Addr::new(100, 128, 1, 2)));
    }
}
