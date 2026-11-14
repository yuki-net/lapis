//! 拡張の目録が Host のデータ領域から読まれ、再起動でも保たれることを確認する。

use extensions_entry::{
    ExtensionCatalog, ExtensionEntry, ExtensionPackageId, ExtensionState, ExtensionVersion,
};
use host::{BackendClient, BackendHost};

fn entry(id: &str, enabled: bool) -> ExtensionEntry {
    ExtensionEntry::new(
        ExtensionPackageId::try_new(id).unwrap(),
        ExtensionVersion::parse("0.2.0").unwrap(),
        ExtensionState::installed(enabled),
    )
}

#[test]
fn a_host_without_a_state_directory_has_no_extensions() {
    let host = BackendHost::start().unwrap();
    assert!(host.extensions().unwrap().extensions.is_empty());
}

#[test]
fn the_catalog_lives_below_the_host_state_directory_and_survives_restart() {
    let state = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(state.path().join("extensions")).unwrap();
    catalog.register(entry("dev.example.one", true)).unwrap();
    catalog.register(entry("dev.example.two", false)).unwrap();
    drop(catalog);

    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    let listed = host.extensions().unwrap().extensions;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, "dev.example.one");
    assert!(listed[0].enabled);
    assert_eq!(listed[1].id, "dev.example.two");
    assert!(listed[1].installed);
    assert!(!listed[1].enabled);
    drop(host);

    // 再起動しても同じ宣言を見せる。
    let restarted = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert_eq!(restarted.extensions().unwrap().extensions, listed);
}

#[test]
fn package_and_data_placements_are_below_the_host_state_directory() {
    let state = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(state.path().join("extensions")).unwrap();
    catalog.register(entry("dev.example.one", true)).unwrap();
    let id = ExtensionPackageId::try_new("dev.example.one").unwrap();

    let extensions_root = state.path().join("extensions");
    assert_eq!(
        catalog.data_directory(&id).unwrap(),
        extensions_root.join("data").join("dev.example.one")
    );
    assert_eq!(
        catalog.installed_directory(&id).unwrap(),
        extensions_root.join("installed").join("dev.example.one")
    );
    assert!(
        catalog
            .data_directory(&id)
            .unwrap()
            .starts_with(state.path()),
        "拡張データの正本は Host のデータ領域に置く"
    );
}

#[test]
fn a_broken_catalog_does_not_stop_the_host_and_is_never_replaced() {
    let state = tempfile::tempdir().unwrap();
    let directory = state.path().join("extensions");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("catalog.json"), b"not json").unwrap();

    let host = BackendHost::start_with_state_directory(state.path().to_owned());
    let host = host.unwrap_or_else(|_| panic!("目録の破損で Host を停止しない"));
    assert!(host.extensions().is_err());
    assert_eq!(
        std::fs::read(directory.join("catalog.json")).unwrap(),
        b"not json"
    );
}

#[test]
fn embedded_and_local_clients_read_the_same_declaration() {
    let state = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(state.path().join("extensions")).unwrap();
    catalog.register(entry("dev.example.one", true)).unwrap();

    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    assert_eq!(
        BackendClient::from(host.clone())
            .extensions(workspace)
            .unwrap()
            .extensions,
        host.extensions().unwrap().extensions
    );
}
