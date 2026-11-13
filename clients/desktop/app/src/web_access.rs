//! ローカル Web へ接続するための候補アドレス収集と、Host の Web 公開管理。
//!
//! アドレスは運用で変わる（Tailscale の IP 変更など）ため、起動時ではなく
//! 表示のために集め直す。ページ配信と bridge のポートは既定値を使う。

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use desktop_client::{
    HostDevice, HostWebStatus, WebAccess, WebAccessControl, WebAddress, WebAddressKind,
    address_kind_for,
};

/// この端末で利用できる Web 接続の候補を返す。
/// Workspace ID は接続中の Workspace から設定画面が補うため、ここでは持たない。
pub(super) fn local_web_access() -> WebAccess {
    WebAccess::new(
        wire::WEB_PAGE_PORT,
        wire::WEB_BRIDGE_PORT,
        interface_addresses(),
        None,
    )
    .with_loopback_fallback()
}

/// loopback 以外の IPv4 アドレスを、表示用の種別と一緒に返す。
fn interface_addresses() -> Vec<WebAddress> {
    let networks = sysinfo::Networks::new_with_refreshed_list();
    let mut addresses: Vec<WebAddress> = vec![WebAddress::new(
        WebAddressKind::Local,
        Ipv4Addr::LOCALHOST.to_string(),
    )];
    for (name, network) in networks.list() {
        for network in network.ip_networks() {
            let IpAddr::V4(ip) = network.addr else {
                continue;
            };
            let Some(kind) = address_kind_for(name, ip) else {
                continue;
            };
            let address = WebAddress::new(kind, ip.to_string());
            if !addresses
                .iter()
                .any(|existing| existing.same_host(&address))
            {
                addresses.push(address);
            }
        }
    }
    addresses
}

/// Desktop から Local Host の Web 公開と端末を管理する操作の実装。
pub(super) struct LocalWebAccess(pub host::BackendClient);

impl LocalWebAccess {
    fn workspace(&self, root: &Path) -> Result<client_api::WorkspaceId, String> {
        self.0
            .register_workspace(root)
            .map_err(|error| error.to_string())
    }
}

impl WebAccessControl for LocalWebAccess {
    fn status(&self, root: &Path) -> Result<HostWebStatus, String> {
        self.0
            .web_status(self.workspace(root)?)
            .map_err(|error| error.to_string())
    }

    fn start(&self, root: &Path, address: &str, port: u16) -> Result<HostWebStatus, String> {
        self.0
            .start_web(self.workspace(root)?, address.to_owned(), port)
            .map_err(|error| error.to_string())
    }

    fn stop(&self, root: &Path) -> Result<HostWebStatus, String> {
        self.0
            .stop_web(self.workspace(root)?)
            .map_err(|error| error.to_string())
    }

    fn invite(&self, root: &Path) -> Result<zeroize::Zeroizing<String>, String> {
        let workspace = self.workspace(root)?;
        let status = self
            .0
            .web_status(workspace.clone())
            .map_err(|error| error.to_string())?;
        let url = status.url.ok_or("Web 公開を開始してください")?;
        let token = self
            .0
            .issue_web_invitation(workspace)
            .map_err(|error| error.to_string())?;
        Ok(zeroize::Zeroizing::new(format!(
            "{}#/invite?token={}",
            url,
            token.as_str()
        )))
    }

    fn devices(&self, root: &Path) -> Result<Vec<HostDevice>, String> {
        self.0
            .managed_devices(self.workspace(root)?)
            .map_err(|error| error.to_string())
    }

    fn revoke(&self, root: &Path, device_id: &str) -> Result<(), String> {
        match self
            .0
            .revoke_managed_device(self.workspace(root)?, device_id.to_owned())
        {
            Ok(true) => Ok(()),
            Ok(false) => Err("端末が見つかりません".into()),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_always_available_for_display() {
        let access = local_web_access();
        assert!(
            access
                .addresses()
                .iter()
                .any(|address| address.host == "127.0.0.1")
        );
        assert_eq!(access.page_port, 4747);
        assert_eq!(access.bridge_port, 4757);
    }
}
