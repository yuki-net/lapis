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
            "pending": [{
                "id": "dev.example.partial",
                "operation": "install",
                "phase": "catalog_written",
                "previous": null,
            }],
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
fn the_host_does_not_publish_declarations_when_recovery_fails() {
    let state = tempfile::tempdir().unwrap();
    let directory = state.path().join("extensions");
    let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
    catalog.register(entry("dev.example.one", true)).unwrap();
    drop(catalog);
    // 解釈できない journal。回復できないため、目録を宣言として配らない。
    std::fs::write(directory.join("journal.json"), b"not json").unwrap();

    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(
        host.extensions().is_err(),
        "回復できない中断を残したまま宣言を配らない"
    );
    assert_eq!(
        std::fs::read(directory.join("journal.json")).unwrap(),
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

#[test]
fn embedded_client_saves_extension_state_for_next_restart() {
    let state = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(state.path().join("extensions")).unwrap();
    catalog.register(entry("dev.example.one", true)).unwrap();
    drop(catalog);
    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let client = BackendClient::from(host.clone());
    let response = client
        .set_extension_enabled(workspace.clone(), "dev.example.one".to_owned(), false)
        .unwrap();
    assert!(!response.enabled);
    assert!(!host.extensions().unwrap().extensions[0].enabled);
    assert!(
        client
            .set_extension_enabled(workspace, "dev.example.missing".to_owned(), false)
            .is_err()
    );
    drop(client);
    drop(host);
    let restarted = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(!restarted.extensions().unwrap().extensions[0].enabled);
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

#[test]
fn host_accepts_explicitly_trusted_external_publisher() {
    use base64::Engine as _;
    use ed25519_dalek::{Signer as _, SigningKey};
    use sha2::Digest as _;
    let state = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[42; 32]);
    let trusted = state.path().join("extensions");
    std::fs::create_dir_all(&trusted).unwrap();
    std::fs::write(
        trusted.join("trusted-keys.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "keys": {"external": base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes())}
        })).unwrap(),
    ).unwrap();
    let bytes = br#"{"schema":1,"settings":[{"definition":{"name":"font-size","value_kind":"integer","scopes":["user"],"default":12,"allowed_values":null},"label":"terminal.font"}],"translations":[{"target":"dev.example.language","locale":"en-US","messages":{"terminal.font":"Font"}}]}"#;
    let mut manifest = serde_json::json!({
        "schema": 1,
        "kind": "extension",
        "id": "dev.example.language",
        "version": "0.1.0",
        "requires": [
            {"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"},
            {"contract": "lapis.extension.contributions", "minimum": "1.0.0", "maximum": "1.0.0"}
        ],
        "files": [{"path": "contributions.json", "sha256": format!("{:x}", sha2::Sha256::digest(bytes))}],
        "key_id": "external"
    });
    fn canonical(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(object) => {
                let mut keys: Vec<_> = object.keys().collect();
                keys.sort_unstable();
                out.push('{');
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(key).unwrap());
                    out.push(':');
                    canonical(&object[key], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    canonical(value, out);
                }
                out.push(']');
            }
            other => out.push_str(&serde_json::to_string(other).unwrap()),
        }
    }
    let mut payload = String::new();
    canonical(&manifest, &mut payload);
    let signature = key.sign(payload.as_bytes());
    manifest["signature"] =
        serde_json::json!(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()));
    let package = write_package(state.path(), &[("contributions.json", bytes)], &manifest);
    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(
        host.install_extension(&package, true)
            .unwrap()
            .is_installed()
    );
    assert!(host.extensions().unwrap().contributions.is_empty());
    assert_eq!(host.extensions().unwrap().extensions[0].active, Some(false));
    drop(host);
    let restarted = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(restarted.extension_startup_errors().is_empty());
    assert_eq!(
        restarted.extensions().unwrap().extensions[0].active,
        Some(true)
    );
    assert_eq!(
        restarted.extensions().unwrap().extensions[0].id,
        "dev.example.language"
    );
    let contributions = restarted.extensions().unwrap().contributions;
    assert_eq!(contributions.len(), 1);
    assert_eq!(contributions[0].extension_id, "dev.example.language");
    assert_eq!(
        contributions[0].declaration.settings[0].label,
        "terminal.font"
    );
    let root = tempfile::tempdir().unwrap();
    let workspace = restarted.register_workspace(root.path()).unwrap();
    assert_eq!(
        BackendClient::from(restarted.clone())
            .extensions(workspace)
            .unwrap()
            .contributions,
        contributions
    );
    restarted
        .set_extension_enabled("dev.example.language", false)
        .unwrap();
    assert_eq!(
        restarted.extensions().unwrap().extensions[0].active,
        Some(true)
    );
    assert!(!restarted.extensions().unwrap().extensions[0].enabled);
    assert_eq!(restarted.extensions().unwrap().contributions, contributions);
    drop(restarted);
    let disabled = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(disabled.extensions().unwrap().contributions.is_empty());
    assert_eq!(
        disabled.extensions().unwrap().extensions[0].active,
        Some(false)
    );
    disabled
        .set_extension_enabled("dev.example.language", true)
        .unwrap();
    assert!(disabled.extensions().unwrap().contributions.is_empty());
    drop(disabled);
    let reenabled = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert_eq!(
        reenabled.extensions().unwrap().extensions[0].active,
        Some(true)
    );
    assert_eq!(reenabled.extensions().unwrap().contributions, contributions);
    drop(reenabled);
    let installed_contribution = trusted.join("installed/dev.example.language/contributions.json");
    std::fs::write(&installed_contribution, b"{}").unwrap();
    let tampered = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(tampered.extensions().unwrap().contributions.is_empty());
    assert!(
        tampered
            .extension_startup_errors()
            .contains_key("dev.example.language")
    );
    drop(tampered);
    std::fs::write(&installed_contribution, bytes).unwrap();
    std::fs::remove_file(trusted.join("trusted-keys.json")).unwrap();
    let no_longer_trusted =
        BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(
        no_longer_trusted
            .extension_startup_errors()
            .contains_key("dev.example.language")
    );
    assert!(no_longer_trusted.extensions().unwrap().extensions[0].installed);
    assert!(
        no_longer_trusted
            .extensions()
            .unwrap()
            .contributions
            .is_empty()
    );
}

#[test]
fn unsigned_package_is_available_only_in_development_mode() {
    use sha2::Digest as _;
    let state = tempfile::tempdir().unwrap();
    let bytes = b"language data";
    let manifest = serde_json::json!({
        "schema": 1,
        "kind": "extension",
        "id": "dev.example.unsigned",
        "version": "0.1.0",
        "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}],
        "files": [{"path": "language.json", "sha256": format!("{:x}", sha2::Sha256::digest(bytes))}]
    });
    let package = write_package(state.path(), &[("language.json", bytes)], &manifest);
    let production = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(production.install_extension(&package, true).is_err());
    drop(production);
    let development =
        BackendHost::start_development_with_state_directory(state.path().to_owned()).unwrap();
    assert!(
        development
            .install_extension(&package, true)
            .unwrap()
            .is_installed()
    );
    drop(development);
    let production = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    assert!(
        production
            .extension_startup_errors()
            .contains_key("dev.example.unsigned")
    );
    drop(production);
    let development =
        BackendHost::start_development_with_state_directory(state.path().to_owned()).unwrap();
    assert!(development.extension_startup_errors().is_empty());
}

#[test]
fn contributions_without_versioned_contract_or_over_the_limit_are_not_published() {
    use sha2::Digest as _;
    let state = tempfile::tempdir().unwrap();
    let host =
        BackendHost::start_development_with_state_directory(state.path().to_owned()).unwrap();
    let make_package = |id: &str, contents: &[u8], requires: serde_json::Value| {
        let manifest = serde_json::json!({
            "schema": 1, "kind": "extension", "id": id, "version": "0.1.0", "requires": requires,
            "files": [{"path": "contributions.json", "sha256": format!("{:x}", sha2::Sha256::digest(contents))}]
        });
        write_package(state.path(), &[("contributions.json", contents)], &manifest)
    };
    let package = make_package(
        "dev.example.no-contract",
        br#"{"schema":1}"#,
        serde_json::json!([
            {"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}
        ]),
    );
    host.install_extension(&package, true).unwrap();
    let oversized = vec![b' '; 256 * 1024 + 1];
    let package = make_package(
        "dev.example.oversized",
        &oversized,
        serde_json::json!([
            {"contract": "lapis.extension.contributions", "minimum": "1.0.0", "maximum": "1.0.0"}
        ]),
    );
    host.install_extension(&package, true).unwrap();
    drop(host);
    let restarted =
        BackendHost::start_development_with_state_directory(state.path().to_owned()).unwrap();
    assert!(restarted.extensions().unwrap().contributions.is_empty());
    assert!(
        restarted
            .extension_startup_errors()
            .contains_key("dev.example.no-contract")
    );
    assert!(
        restarted
            .extension_startup_errors()
            .contains_key("dev.example.oversized")
    );
}
