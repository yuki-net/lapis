//! managed Host が Web 公開プロセスを所有する。
use std::{
    io::{BufRead, BufReader, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    time::Duration,
};

use client_api::HostWebStatus;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationConfig {
    version: u32,
    address: Option<SocketAddr>,
}

struct Running {
    child: Child,
    address: SocketAddr,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) struct WebPublisher {
    connection: tempfile::NamedTempFile,
    executable: PathBuf,
    dist: PathBuf,
    running: Mutex<Option<Running>>,
    config: PathBuf,
    last_error: Mutex<Option<String>>,
}

impl WebPublisher {
    pub(crate) fn new(
        directory: &Path,
        connection: &wire::HostConnection,
    ) -> std::io::Result<Self> {
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(&mut file, connection)?;
        file.flush()?;
        let executable_dir = std::env::current_exe()?
            .parent()
            .ok_or_else(|| std::io::Error::other("実行ファイルの場所が不明です"))?
            .to_path_buf();
        let executable = std::env::var_os("LAPIS_WEB_BRIDGE_EXECUTABLE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                executable_dir.join(format!("lapis-web-bridge{}", std::env::consts::EXE_SUFFIX))
            });
        let dist = std::env::var_os("LAPIS_WEB_DIST")
            .map(PathBuf::from)
            .unwrap_or_else(|| executable_dir.join("web"));
        Ok(Self {
            connection: file,
            executable,
            dist,
            running: Mutex::new(None),
            config: directory.join("web-publication.json"),
            last_error: Mutex::new(None),
        })
    }

    /// 最後に明示された公開状態を Host の再起動後に復元する。
    pub(crate) fn restore(&self) {
        let result = match std::fs::read(&self.config) {
            Ok(bytes) => serde_json::from_slice::<PublicationConfig>(&bytes)
                .map_err(|_| "Web 公開設定を読み込めません".to_owned())
                .and_then(|config| {
                    if config.version != 1 {
                        return Err("Web 公開設定の版が不正です".into());
                    }
                    match config.address {
                        Some(address) => self.start(address).map(|_| ()),
                        None => Ok(()),
                    }
                }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("Web 公開設定を読み込めません".into()),
        };
        if let Err(error) = result
            && let Ok(mut last_error) = self.last_error.lock()
        {
            *last_error = Some(error);
        }
    }

    fn save_config(&self, address: Option<SocketAddr>) -> Result<(), String> {
        let parent = self.config.parent().ok_or("Web 公開設定を保存できません")?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "Web 公開設定を保存できません")?;
        serde_json::to_writer(
            &mut temporary,
            &PublicationConfig {
                version: 1,
                address,
            },
        )
        .map_err(|_| "Web 公開設定を保存できません")?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|_| "Web 公開設定を保存できません")?;
        temporary
            .persist(&self.config)
            .map_err(|_| "Web 公開設定を保存できません")?;
        Ok(())
    }

    pub(crate) fn status(&self) -> Result<HostWebStatus, String> {
        let mut running = self
            .running
            .lock()
            .map_err(|_| "Web 公開状態を取得できません")?;
        if let Some(current) = running.as_mut()
            && current
                .child
                .try_wait()
                .map_err(|_| "Web 公開状態を取得できません")?
                .is_some()
        {
            *running = None;
            *self
                .last_error
                .lock()
                .map_err(|_| "Web 公開状態を取得できません")? =
                Some("Web 公開プロセスが終了しました".into());
        }
        Ok(HostWebStatus {
            url: running
                .as_ref()
                .map(|current| format!("http://{}/", current.address)),
            addresses: addresses(),
            error: self
                .last_error
                .lock()
                .map_err(|_| "Web 公開状態を取得できません")?
                .clone(),
        })
    }

    pub(crate) fn start(&self, address: SocketAddr) -> Result<HostWebStatus, String> {
        if !allowed_ip(address.ip()) {
            return Err("同一 PC または Tailscale のアドレスを指定してください".into());
        }
        if !addresses()
            .iter()
            .any(|candidate| candidate == &address.ip().to_string())
        {
            return Err("この端末に存在するアドレスを指定してください".into());
        }
        if !self.dist.join("index.html").is_file() {
            return Err("Web の配信ファイルがありません".into());
        }
        let mut running = self
            .running
            .lock()
            .map_err(|_| "Web 公開を開始できません")?;
        if let Some(current) = running.as_mut() {
            if current
                .child
                .try_wait()
                .map_err(|_| "Web 公開状態を取得できません")?
                .is_none()
            {
                return Err("Web は既に公開中です".into());
            }
            *running = None;
        }
        let mut command = Command::new(&self.executable);
        command
            .arg("--managed")
            .arg("--connection-info")
            .arg(self.connection.path())
            .arg("--listen")
            .arg(address.to_string())
            .arg("--dist")
            .arg(&self.dist)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if !address.ip().is_loopback() {
            command.arg("--allow-remote");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|_| "Web 公開を起動できません")?;
        let output = child
            .stdout
            .take()
            .ok_or("Web 公開の起動結果がありません")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(output).read_line(&mut line).map(|_| line);
            let _ = sender.send(result);
        });
        let ready = receiver
            .recv_timeout(Duration::from_secs(15))
            .ok()
            .and_then(Result::ok)
            .and_then(|line| {
                line.trim()
                    .strip_prefix("ready=")
                    .and_then(|text| text.parse::<SocketAddr>().ok())
            });
        let Some(bound) = ready.filter(|bound| {
            bound.ip() == address.ip() && (address.port() == 0 || bound.port() == address.port())
        }) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Web 公開の起動に失敗しました".into());
        };
        let status = HostWebStatus {
            url: Some(format!("http://{bound}/")),
            addresses: addresses(),
            error: None,
        };
        let started = Running {
            child,
            address: bound,
        };
        self.save_config(Some(bound))?;
        *running = Some(started);
        *self
            .last_error
            .lock()
            .map_err(|_| "Web 公開状態を保存できません")? = None;
        Ok(status)
    }

    pub(crate) fn stop(&self) -> Result<HostWebStatus, String> {
        let mut running = self
            .running
            .lock()
            .map_err(|_| "Web 公開を停止できません")?;
        // 設定保存に失敗しても公開プロセスは必ず止める。
        *running = None;
        drop(running);
        if let Err(error) = self.save_config(None) {
            if let Ok(mut last_error) = self.last_error.lock() {
                *last_error = Some(error.clone());
            }
            return Err(error);
        }
        *self
            .last_error
            .lock()
            .map_err(|_| "Web 公開状態を保存できません")? = None;
        Ok(HostWebStatus {
            url: None,
            addresses: addresses(),
            error: None,
        })
    }
}

fn allowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip == Ipv4Addr::LOCALHOST
                || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
        }
        IpAddr::V6(_) => false,
    }
}

fn addresses() -> Vec<String> {
    let tailscale = tailscale_ipv4();
    let networks = sysinfo::Networks::new_with_refreshed_list();
    let interfaces = networks.list().iter().flat_map(|(name, network)| {
        network
            .ip_networks()
            .iter()
            .map(move |address| (name.as_str(), address.addr))
    });
    candidate_addresses(tailscale, interfaces)
}

fn candidate_addresses<'a>(
    tailscale: Option<IpAddr>,
    interfaces: impl Iterator<Item = (&'a str, IpAddr)>,
) -> Vec<String> {
    let mut result = vec![Ipv4Addr::LOCALHOST.to_string()];
    if let Some(ip) = tailscale
        && allowed_ip(ip)
        && !ip.is_loopback()
    {
        result.push(ip.to_string());
    }
    for (name, address) in interfaces {
        if is_tailscale_address(name, address, tailscale) && !result.contains(&address.to_string())
        {
            result.push(address.to_string());
        }
    }
    result
}

fn is_tailscale_address(name: &str, ip: IpAddr, reported: Option<IpAddr>) -> bool {
    allowed_ip(ip)
        && !ip.is_loopback()
        && (name.to_ascii_lowercase().contains("tailscale") || reported == Some(ip))
}

/// 他の CGNAT 網を Tailscale と誤認しないよう、CLI の報告する IP も照合する。
fn tailscale_ipv4() -> Option<IpAddr> {
    use std::io::Read;
    let mut command = Command::new("tailscale");
    command
        .args(["ip", "-4"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut output = String::new();
                child
                    .stdout
                    .take()?
                    .take(128)
                    .read_to_string(&mut output)
                    .ok()?;
                return output
                    .lines()
                    .next()?
                    .trim()
                    .parse::<Ipv4Addr>()
                    .ok()
                    .map(IpAddr::V4);
            }
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_binding_is_limited_to_loopback_and_tailscale() {
        for ip in ["127.0.0.1", "100.64.0.1", "100.127.255.254"] {
            assert!(allowed_ip(ip.parse().unwrap()));
        }
        for ip in ["0.0.0.0", "192.168.1.1", "100.63.0.1", "100.128.0.1", "::1"] {
            assert!(!allowed_ip(ip.parse().unwrap()));
        }
        let address = "100.100.1.2".parse().unwrap();
        assert!(!is_tailscale_address("Ethernet", address, None));
        assert!(is_tailscale_address("Ethernet", address, Some(address)));
        assert!(is_tailscale_address("Tailscale", address, None));
        assert_eq!(
            candidate_addresses(Some(address), std::iter::empty()),
            vec!["127.0.0.1", "100.100.1.2"]
        );
        assert_eq!(
            candidate_addresses(None, [("Ethernet", address)].into_iter()),
            vec!["127.0.0.1"]
        );
    }
}
