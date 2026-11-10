//! 接続中の Host / Workspace を利用者へ示す表示文言。
//!
//! Web の接続情報と Desktop のワークスペースメニューで同じ内容を出すため、
//! 整形だけをここに置き、表示先ごとの組み立てを持たない。

/// Host URL から利用者へ見せる Host 名を取り出す。
///
/// URL として解釈できない値でも、空白を除いた元の文字列を表示に使う。
pub fn host_name_from_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return "Host".to_owned();
    }
    let after_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .trim();
    if host.is_empty() {
        trimmed.to_owned()
    } else {
        host.to_owned()
    }
}

/// Host 名と ID を表示する。ID が無い場合はその旨を示す。
pub fn host_label(host_name: Option<&str>, host_id: Option<&str>) -> String {
    let name = non_empty(host_name).unwrap_or("Host");
    let id = non_empty(host_id).unwrap_or("未永続化");
    format!("Host {name} ({id})")
}

/// Workspace 名と ID を表示する。名前が無い場合は ID を名前として使う。
pub fn workspace_label(workspace_name: Option<&str>, workspace_id: Option<&str>) -> String {
    let id = non_empty(workspace_id).unwrap_or("未選択");
    let name = non_empty(workspace_name).unwrap_or(id);
    format!("Workspace {name} ({id})")
}

/// Web と Desktop で共通の接続情報表示。
pub fn connection_label(
    host_name: Option<&str>,
    host_id: Option<&str>,
    workspace_name: Option<&str>,
    workspace_id: Option<&str>,
) -> String {
    format!(
        "{} / {}",
        host_label(host_name, host_id),
        workspace_label(workspace_name, workspace_id)
    )
}

/// タブ・ヘッダー・ウィンドウタイトルへ出す短い Workspace 名。
///
/// 接続先 URL と Workspace ID は含めない。詳細な identity は
/// [`connection_label`] を表示する接続情報の面が担う。
pub fn short_workspace_label(workspace_name: Option<&str>, workspace_id: Option<&str>) -> String {
    let name = workspace_name
        .map(|name| name.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|name| !name.is_empty());
    let id = non_empty(workspace_id);
    match name {
        Some(name) if id.is_none_or(|id| name != id) => name,
        _ => "Workspace".to_owned(),
    }
}

/// Desktop のウィンドウタイトル。Web のタブタイトルと同じ形式にする。
pub fn window_title(workspace_name: Option<&str>, workspace_id: Option<&str>) -> String {
    format!(
        "{} - Lapis",
        short_workspace_label(workspace_name, workspace_id)
    )
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_name_uses_the_url_authority() {
        assert_eq!(
            host_name_from_url("ws://127.0.0.1:4757/remote"),
            "127.0.0.1:4757"
        );
        assert_eq!(
            host_name_from_url("wss://user@example.com/lapis?workspace=1"),
            "example.com"
        );
        assert_eq!(host_name_from_url("  localhost:4787  "), "localhost:4787");
        assert_eq!(host_name_from_url(""), "Host");
    }

    #[test]
    fn labels_include_names_and_ids_and_fallbacks() {
        assert_eq!(
            host_label(Some("my-host"), Some("host-1")),
            "Host my-host (host-1)"
        );
        assert_eq!(host_label(Some("my-host"), None), "Host my-host (未永続化)");
        assert_eq!(
            workspace_label(Some("repo"), Some("workspace-1")),
            "Workspace repo (workspace-1)"
        );
        assert_eq!(
            workspace_label(None, Some("workspace-1")),
            "Workspace workspace-1 (workspace-1)"
        );
        assert_eq!(
            connection_label(
                Some("my-host"),
                Some("host-1"),
                Some("repo"),
                Some("workspace-1")
            ),
            "Host my-host (host-1) / Workspace repo (workspace-1)"
        );
    }

    #[test]
    fn short_labels_do_not_include_urls_or_ids() {
        assert_eq!(
            short_workspace_label(Some("repo"), Some("workspace-1")),
            "repo"
        );
        assert_eq!(
            short_workspace_label(Some("workspace-1"), Some("workspace-1")),
            "Workspace"
        );
        assert_eq!(
            short_workspace_label(Some("  "), Some("workspace-1")),
            "Workspace"
        );
        assert_eq!(short_workspace_label(None, None), "Workspace");
        assert_eq!(
            window_title(Some("repo"), Some("workspace-1")),
            "repo - Lapis"
        );
        assert_eq!(window_title(None, None), "Workspace - Lapis");
    }
}
