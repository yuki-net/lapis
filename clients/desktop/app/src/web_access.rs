//! ローカル Web へ接続するための、この端末の候補アドレスを集める。
//!
//! アドレスは運用で変わる（Tailscale の IP 変更など）ため、起動時ではなく
//! 表示のたびに集め直す。ページ配信と bridge のポートは既定値を使う。

use std::net::{IpAddr, Ipv4Addr};

use desktop_client::{WebAccess, WebAddress, WebAddressKind, address_kind_for};

/// この端末で利用できる Web 接続の候補を返す。
///
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
