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

/// 署名済みの `.lapis` package を作る。repository の署名鍵は持たないため、Host の
/// 読み込み経路のうち目録・配置だけを確かめるときに使う。
fn write_package(
    directory: &std::path::Path,
    files: &[(&str, &[u8])],
    manifest: &serde_json::Value,
) -> std::path::PathBuf {
    use std::io::Write as _;
    let path = directory.join("package.lapis");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("manifest.json", options).unwrap();
    writer
        .write_all(&serde_json::to_vec(manifest).unwrap())
        .unwrap();
    for (name, bytes) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    path
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
fn an_interrupted_install_is_recovered_before_the_host_publishes_it() {
    let state = tempfile::tempdir().unwrap();
    let directory = state.path().join("extensions");
    let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
    // 実体を置く前に落ちた「導入済み」の宣言。
    catalog
        .register(entry("dev.example.partial", true))
        .unwrap();
    drop(catalog);
    // 中断した導入の記録。取り消し対象は新規導入（previous なし）。
    std::fs::write(
        directory.join("journal.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "pending": [{ "id": "dev.example.partial", "previous": null }],
        }))
        .unwrap(),
    )
    .unwrap();

    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(
        host.extensions().unwrap().extensions.is_empty(),
        "実体の無い拡張を宣言として配らない"
    );
    assert!(!directory.join("journal.json").exists());
    assert!(
        ExtensionCatalog::open(directory)
            .unwrap()
            .get(&ExtensionPackageId::try_new("dev.example.partial").unwrap())
            .is_none()
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

#[test]
fn an_incompatible_extension_is_declared_apart_from_installed_ones() {
    let state = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(state.path().join("extensions")).unwrap();
    catalog.register(entry("dev.example.one", true)).unwrap();
    catalog
        .register(ExtensionEntry::new(
            ExtensionPackageId::try_new("dev.example.future").unwrap(),
            ExtensionVersion::parse("1.0.0").unwrap(),
            ExtensionState::incompatible(),
        ))
        .unwrap();
    drop(catalog);

    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    let listed = host.extensions().unwrap().extensions;
    let installed = listed
        .iter()
        .find(|extension| extension.id == "dev.example.one")
        .unwrap();
    assert!(installed.installed && installed.enabled && !installed.incompatible);
    let future = listed
        .iter()
        .find(|extension| extension.id == "dev.example.future")
        .unwrap();
    assert!(!future.installed && !future.enabled && future.incompatible);
}

#[test]
fn enabling_and_disabling_change_only_the_declaration_until_restart() {
    let state = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(state.path().join("extensions")).unwrap();
    catalog.register(entry("dev.example.one", true)).unwrap();
    drop(catalog);

    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(
        host.set_extension_enabled("dev.example.one", false)
            .unwrap()
            .is_some()
    );
    assert!(!host.extensions().unwrap().extensions[0].enabled);

    // 未導入の拡張は有効にできない。
    assert!(
        host.set_extension_enabled("dev.example.none", true)
            .unwrap()
            .is_none()
    );

    // 再起動すると目録から同じ宣言を読む。
    drop(host);
    let restarted = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    let listed = restarted.extensions().unwrap().extensions;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].installed);
    assert!(!listed[0].enabled);
}

#[test]
fn a_package_without_a_trusted_signature_is_rejected_and_not_declared() {
    let state = tempfile::tempdir().unwrap();
    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    let manifest = serde_json::json!({
        "schema": 1,
        "kind": "extension",
        "id": "dev.example.unsigned",
        "version": "0.1.0",
        "requires": [{
            "contract": "lapis.extension",
            "minimum": "1.0.0",
            "maximum": "1.0.0",
        }],
        "files": [{
            "path": "extension.wasm",
            "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        }],
        "key_id": "not-a-trusted-key",
        "signature": "AA==",
    });
    let package = write_package(state.path(), &[("extension.wasm", b"\0asm")], &manifest);

    let error = host
        .install_extension(&package, true)
        .expect_err("信頼していない署名の package は読み込まない");
    assert!(error.contains("署名"), "{error}");
    assert!(
        host.extensions().unwrap().extensions.is_empty(),
        "署名が不正な package は目録に載せない"
    );
    assert!(
        !state
            .path()
            .join("extensions")
            .join("installed")
            .join("dev.example.unsigned")
            .exists()
    );
}
