//! ローカル Web（ページ配信と bridge）へ接続するための表示情報。
//!
//! アドレスは運用で変わる（Tailscale の IP 変更など）ため、保存値ではなく
//! 表示のたびに「いま使える候補」から組み立てる。整形だけをここに置き、
//! 候補の収集（NIC の列挙や bridge の待受）は platform 側の責務にする。

/// Web ページ配信と bridge のアドレス種別。表示順もこの並び。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WebAddressKind {
    /// この端末自身（loopback）。
    Local,
    /// 同じ LAN。
    Network,
    /// Tailscale の tailnet。
    Tailscale,
}

impl WebAddressKind {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Local => "settings.web.kind.local",
            Self::Network => "settings.web.kind.network",
            Self::Tailscale => "settings.web.kind.tailscale",
        }
    }
}

/// 候補アドレス。`host` は `127.0.0.1` や `100.64.1.2` のような host 部だけを持つ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebAddress {
    pub kind: WebAddressKind,
    pub host: String,
    /// ページをこの scheme で開くか。Web で https 配信している場合だけ `https`。
    pub page_scheme: PageScheme,
}

/// ページ URL の scheme。bridge は常に `ws` / `wss` をページの scheme から決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageScheme {
    Http,
    Https,
}

impl PageScheme {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    fn websocket_scheme(self) -> &'static str {
        match self {
            Self::Http => "ws",
            Self::Https => "wss",
        }
    }
}

impl WebAddress {
    pub fn new(kind: WebAddressKind, host: impl Into<String>) -> Self {
        Self {
            kind,
            host: host.into(),
            page_scheme: PageScheme::Http,
        }
    }

    /// ブラウザーで開くページ URL。末尾は `/`。
    pub fn page_url(&self, page_port: u16) -> String {
        format!("{}://{}:{page_port}/", self.page_scheme.as_str(), self.host)
    }

    /// Web の Host URL。ログインフォームへ入れる bridge の WebSocket。
    ///
    /// https のページからは混在コンテンツを避けるため `wss` を使う。
    pub fn bridge_url(&self, bridge_port: u16) -> String {
        format!(
            "{}://{}:{bridge_port}{}",
            self.page_scheme.websocket_scheme(),
            self.host,
            wire_path()
        )
    }

    /// 同じホストの候補か。ポート違いや重複を除くために使う。
    pub fn same_host(&self, other: &Self) -> bool {
        self.host.eq_ignore_ascii_case(&other.host)
    }
}

fn wire_path() -> &'static str {
    wire::REMOTE_WEBSOCKET_PATH
}

/// 設定画面に出す接続情報一式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebAccess {
    pub page_port: u16,
    pub bridge_port: u16,
    pub addresses: Vec<WebAddress>,
    /// bridge が接続情報から読んだ Workspace ID。未接続なら空。
    pub workspace_id: Option<String>,
}

impl WebAccess {
    pub fn new(
        page_port: u16,
        bridge_port: u16,
        addresses: Vec<WebAddress>,
        workspace_id: Option<String>,
    ) -> Self {
        Self {
            page_port,
            bridge_port,
            addresses: deduplicate(addresses),
            workspace_id: workspace_id.filter(|id| !id.trim().is_empty()),
        }
    }

    /// 表示に使う候補。ローカルを先頭に、LAN、Tailscale の順へ並べる。
    pub fn addresses(&self) -> &[WebAddress] {
        &self.addresses
    }

    /// 既定で選ぶ候補。ローカルが無ければ先頭。
    pub fn preferred(&self) -> Option<&WebAddress> {
        self.addresses
            .iter()
            .find(|address| address.kind == WebAddressKind::Local)
            .or_else(|| self.addresses.first())
    }

    /// 候補が空でも、この端末自身の loopback を表示できるようにする。
    /// ページ配信や bridge が別端末で動いていても入力すべき URL は変わらない。
    pub fn with_loopback_fallback(mut self) -> Self {
        if self.addresses.is_empty() {
            self.addresses
                .push(WebAddress::new(WebAddressKind::Local, "127.0.0.1"));
        }
        self
    }

    /// ブラウザーで開いているページと接続中の Host URL から表示情報を作る。
    ///
    /// Web ではページ自身が接続先なので、候補はページのホスト1つで足りる。
    pub fn from_urls(
        page_url: Option<&str>,
        host_url: Option<&str>,
        workspace_id: Option<String>,
    ) -> Self {
        let page = page_url.and_then(split_url);
        let bridge = host_url.and_then(split_url);
        let page_port = page
            .as_ref()
            .and_then(|(_, port, _)| *port)
            .unwrap_or(wire::WEB_PAGE_PORT);
        let bridge_port = bridge
            .as_ref()
            .and_then(|(_, port, _)| *port)
            .unwrap_or(wire::WEB_BRIDGE_PORT);
        let addresses = page
            .map(|(host, _, scheme)| {
                let kind = if is_local_host(&host) {
                    WebAddressKind::Local
                } else {
                    WebAddressKind::Network
                };
                let mut address = WebAddress::new(kind, host);
                address.page_scheme = scheme;
                vec![address]
            })
            .unwrap_or_default();
        Self::new(page_port, bridge_port, addresses, workspace_id)
    }
}

/// `http://host:port/path` や `ws://host:port/remote` から host と port を取り出す。
pub fn split_host_port(url: &str) -> Option<(String, Option<u16>)> {
    split_url(url).map(|(host, port, _)| (host, port))
}

/// scheme・host・port をまとめて取り出す。https / wss だけを secure として扱う。
pub fn split_url(url: &str) -> Option<(String, Option<u16>, PageScheme)> {
    let trimmed = url.trim();
    let after_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next()?.trim();
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, rest)| rest);
    if authority.is_empty() {
        return None;
    }
    // IPv6 は `[::1]:4757` の形で来る。角括弧の外にだけ port があるため、
    // 閉じ括弧より後ろの ':' を区切りにする。
    let (host, port) = match authority.rsplit_once(']') {
        Some((inside, after)) => match after.strip_prefix(':') {
            Some(port) => (format!("{inside}]"), Some(port)),
            None => (authority.to_owned(), None),
        },
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (authority.to_owned(), None),
        },
    };
    // port として解釈できない値は port なしとして扱い、host を壊さない。
    let port = port.and_then(|port| port.parse::<u16>().ok());
    let scheme = match url.split_once("://").map(|(scheme, _)| scheme) {
        Some(scheme)
            if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") =>
        {
            PageScheme::Https
        }
        _ => PageScheme::Http,
    };
    Some((host, port, scheme))
}

fn is_local_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn deduplicate(addresses: Vec<WebAddress>) -> Vec<WebAddress> {
    let mut unique: Vec<WebAddress> = Vec::new();
    for address in addresses {
        if address.host.trim().is_empty() {
            continue;
        }
        if unique.iter().any(|existing| existing.same_host(&address)) {
            continue;
        }
        unique.push(address);
    }
    unique.sort_by_key(|address| address.kind);
    unique
}

/// Tailscale の CGNAT 範囲（100.64.0.0/10）か、名前が Tailscale の NIC か。
pub fn is_tailscale_interface(interface: &str, ip: std::net::Ipv4Addr) -> bool {
    interface.to_ascii_lowercase().contains("tailscale")
        || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
}

/// 端末の IPv4 候補を、表示用の種別と一緒に返す。
///
/// loopback は常に含め、LAN と Tailscale は到達可能なものだけを足す。
pub fn address_kind_for(interface: &str, ip: std::net::Ipv4Addr) -> Option<WebAddressKind> {
    if ip.is_loopback() {
        return Some(WebAddressKind::Local);
    }
    if ip.is_link_local() || ip.is_unspecified() {
        return None;
    }
    if is_tailscale_interface(interface, ip) {
        return Some(WebAddressKind::Tailscale);
    }
    Some(WebAddressKind::Network)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_use_the_bridge_websocket_path_and_page_slash() {
        let address = WebAddress::new(WebAddressKind::Tailscale, "100.64.1.23");
        assert_eq!(address.page_url(4747), "http://100.64.1.23:4747/");
        assert_eq!(address.bridge_url(4757), "ws://100.64.1.23:4757/remote");
    }

    #[test]
    fn candidates_are_deduplicated_and_ordered_by_kind() {
        let access = WebAccess::new(
            4747,
            4757,
            vec![
                WebAddress::new(WebAddressKind::Tailscale, "100.64.1.23"),
                WebAddress::new(WebAddressKind::Network, "192.168.1.20"),
                WebAddress::new(WebAddressKind::Local, "127.0.0.1"),
                WebAddress::new(WebAddressKind::Local, "127.0.0.1"),
            ],
            Some("workspace-1".to_owned()),
        );
        assert_eq!(
            access
                .addresses()
                .iter()
                .map(|address| address.host.as_str())
                .collect::<Vec<_>>(),
            vec!["127.0.0.1", "192.168.1.20", "100.64.1.23"]
        );
        assert_eq!(
            access.preferred().map(|address| address.host.as_str()),
            Some("127.0.0.1")
        );
    }

    #[test]
    fn empty_workspace_id_is_not_displayed() {
        let access = WebAccess::new(
            4747,
            4757,
            vec![WebAddress::new(WebAddressKind::Local, "127.0.0.1")],
            Some("  ".to_owned()),
        );
        assert_eq!(access.workspace_id, None);
    }

    #[test]
    fn loopback_fallback_keeps_the_expected_url() {
        let access = WebAccess::new(4747, 4757, Vec::new(), None).with_loopback_fallback();
        assert_eq!(access.addresses().len(), 1);
        assert_eq!(
            access.addresses()[0].page_url(4747),
            "http://127.0.0.1:4747/"
        );
    }

    #[test]
    fn tailscale_detection_covers_cgnat_and_nic_names() {
        assert!(is_tailscale_interface(
            "Ethernet",
            std::net::Ipv4Addr::new(100, 64, 1, 2)
        ));
        assert!(is_tailscale_interface(
            "Tailscale",
            std::net::Ipv4Addr::new(192, 168, 1, 2)
        ));
        assert!(!is_tailscale_interface(
            "Ethernet",
            std::net::Ipv4Addr::new(100, 128, 1, 2)
        ));
        assert_eq!(
            address_kind_for("Tunnel", std::net::Ipv4Addr::new(100, 100, 1, 2)),
            Some(WebAddressKind::Tailscale)
        );
        assert_eq!(
            address_kind_for("Ethernet", std::net::Ipv4Addr::new(192, 168, 1, 2)),
            Some(WebAddressKind::Network)
        );
        assert_eq!(
            address_kind_for("Loopback", std::net::Ipv4Addr::new(127, 0, 0, 1)),
            Some(WebAddressKind::Local)
        );
        assert_eq!(
            address_kind_for("Ethernet", std::net::Ipv4Addr::new(169, 254, 1, 2)),
            None
        );
    }

    #[test]
    fn addresses_are_added_after_dedup_without_ports() {
        let access = WebAccess::new(
            4747,
            4757,
            vec![WebAddress::new(WebAddressKind::Local, "127.0.0.1")],
            None,
        );
        assert!(
            access
                .addresses()
                .iter()
                .all(|address| !address.host.contains(':'))
        );
    }

    #[test]
    fn urls_supply_the_page_and_bridge_ports() {
        let access = WebAccess::from_urls(
            Some("http://100.64.1.23:4747/repo"),
            Some("ws://100.64.1.23:4757/remote"),
            Some("workspace-1".to_owned()),
        );
        assert_eq!(access.page_port, 4747);
        assert_eq!(access.bridge_port, 4757);
        assert_eq!(access.addresses().len(), 1);
        let address = &access.addresses()[0];
        assert_eq!(address.host, "100.64.1.23");
        assert_eq!(address.page_url(4747), "http://100.64.1.23:4747/");
        assert_eq!(address.bridge_url(4757), "ws://100.64.1.23:4757/remote");
        assert_eq!(access.workspace_id.as_deref(), Some("workspace-1"));
    }

    #[test]
    fn https_pages_keep_https_and_use_wss_for_the_bridge() {
        let access = WebAccess::from_urls(
            Some("https://lapis.example:4747/repo"),
            Some("wss://lapis.example:4757/remote"),
            None,
        );
        let address = &access.addresses()[0];
        assert_eq!(address.page_scheme, PageScheme::Https);
        assert_eq!(address.page_url(4747), "https://lapis.example:4747/");
        assert_eq!(address.bridge_url(4757), "wss://lapis.example:4757/remote");
    }

    #[test]
    fn plain_pages_use_http_and_ws() {
        let access = WebAccess::from_urls(Some("http://127.0.0.1:4747/"), None, None);
        let address = &access.addresses()[0];
        assert_eq!(address.page_scheme, PageScheme::Http);
        assert_eq!(address.bridge_url(4757), "ws://127.0.0.1:4757/remote");
    }

    #[test]
    fn loopback_pages_are_labelled_as_this_device() {
        let access = WebAccess::from_urls(Some("http://localhost:4747/"), None, None);
        assert_eq!(access.addresses()[0].kind, WebAddressKind::Local);
        assert_eq!(access.bridge_port, wire::WEB_BRIDGE_PORT);
    }

    #[test]
    fn host_parsing_ignores_credentials_ipv6_and_paths() {
        assert_eq!(
            split_host_port("ws://user:pass@example.com:4757/remote"),
            Some(("example.com".to_owned(), Some(4757)))
        );
        assert_eq!(
            split_host_port("http://[::1]:4747/"),
            Some(("[::1]".to_owned(), Some(4747)))
        );
        // 角括弧付きの IPv6 は port を省けば host をそのまま残す。
        assert_eq!(
            split_host_port("http://[::1]/"),
            Some(("[::1]".to_owned(), None))
        );
        assert_eq!(
            split_host_port("http://100.64.1.23/"),
            Some(("100.64.1.23".to_owned(), None))
        );
        // port が数値でない場合は host を壊さず port なしとして扱う。
        assert_eq!(
            split_host_port("http://example.com:/"),
            Some(("example.com".to_owned(), None))
        );
        assert_eq!(split_host_port("  "), None);
    }
}
