//! `.lapis` package の読み込み。正常系と、非互換・署名不正の package が読み込まれないことを
//! 確かめる。
//!
//! fixture の package は test 内で Ed25519 署名して作る。repository の署名鍵を持たない
//! ため、公開鍵 registry も test の鍵で作る。

use std::path::Path;

use ed25519_dalek::{Signer as _, SigningKey};
use extensions_entry::{
    ContractId, EXTENSION_CONTRACT, ExtensionCatalog, ExtensionEntry, ExtensionLoader,
    ExtensionPackageId, ExtensionState, ExtensionVersion, HOST_EXTENSION_CONTRACT_VERSION,
    HostContracts, InstallOutcome, MANIFEST_NAME, PACKAGE_SCHEMA, PackageError, PackageKind,
    TrustedKeys,
};
use serde_json::{Value, json};

const SEED: [u8; 32] = [11u8; 32];
const KEY_ID: &str = "extension-test";

fn public_key() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .encode(SigningKey::from_bytes(&SEED).verifying_key().to_bytes())
}

fn trusted_keys() -> TrustedKeys {
    TrustedKeys::parse(
        format!(r#"{{"schema":1,"keys":{{"{KEY_ID}":"{}"}}}}"#, public_key()).as_bytes(),
    )
    .unwrap()
}

fn loader() -> ExtensionLoader {
    ExtensionLoader::new(trusted_keys(), HostContracts::current())
}

fn host_version() -> String {
    let (major, minor, patch) = HOST_EXTENSION_CONTRACT_VERSION;
    format!("{major}.{minor}.{patch}")
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical(value: &Value) -> Vec<u8> {
    // `signature` を除き、key を sort して区切りを詰めた JSON。
    let mut sorted = String::new();
    write_sorted(value, &mut sorted);
    sorted.into_bytes()
}

fn write_sorted(value: &Value, output: &mut String) {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_unstable();
            output.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).unwrap());
                output.push(':');
                write_sorted(&object[key], output);
            }
            output.push('}');
        }
        Value::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_sorted(item, output);
            }
            output.push(']');
        }
        other => output.push_str(&serde_json::to_string(other).unwrap()),
    }
}

/// manifest を組み立てて署名する。`mutate` は署名前に manifest を書き換える。
fn signed_manifest(
    kind: &str,
    id: &str,
    requires: &[(&str, &str, &str)],
    files: &[(&str, &[u8])],
    mutate: impl FnOnce(&mut Value),
) -> Value {
    use base64::Engine as _;
    let mut manifest = json!({
        "schema": PACKAGE_SCHEMA,
        "kind": kind,
        "id": id,
        "version": "0.2.0",
        "requires": requires
            .iter()
            .map(|(contract, minimum, maximum)| json!({
                "contract": contract,
                "minimum": minimum,
                "maximum": maximum,
            }))
            .collect::<Vec<_>>(),
        "files": files
            .iter()
            .map(|(path, bytes)| json!({ "path": path, "sha256": sha256(bytes) }))
            .collect::<Vec<_>>(),
        "key_id": KEY_ID,
    });
    mutate(&mut manifest);
    let signature = SigningKey::from_bytes(&SEED).sign(&canonical(&manifest));
    manifest["signature"] =
        json!(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()));
    manifest
}

fn extension_manifest(id: &str, requires: &[(&str, &str, &str)]) -> Value {
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    signed_manifest("extension", id, requires, files, |_| {})
}

fn write_package(
    directory: &Path,
    name: &str,
    manifest: &Value,
    files: &[(&str, &[u8])],
) -> std::path::PathBuf {
    use std::io::Write as _;
    let path = directory.join(name);
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file(MANIFEST_NAME, options).unwrap();
    writer
        .write_all(&serde_json::to_vec_pretty(manifest).unwrap())
        .unwrap();
    for (path, bytes) in files {
        writer.start_file(*path, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    path
}

fn id(value: &str) -> ExtensionPackageId {
    ExtensionPackageId::try_new(value).unwrap()
}

#[test]
fn an_extension_package_is_verified_extracted_and_declared() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let manifest = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "terminal.lapis", &manifest, files);

    let outcome = loader()
        .install(&mut catalog, &package, true)
        .expect("signed and compatible");
    assert_eq!(
        outcome,
        InstallOutcome::Installed {
            id: id("dev.example.terminal"),
            version: ExtensionVersion::parse("0.2.0").unwrap(),
            enabled: true,
        }
    );
    assert!(outcome.is_installed());

    // 宣言は目録の状態を写す。
    let entry = catalog.get(&id("dev.example.terminal")).unwrap();
    assert!(entry.state.is_installed());
    assert!(entry.state.is_enabled());
    assert!(!entry.state.is_incompatible());

    // 実体は導入済みの置き場所へ展開し、拡張データの置き場所も用意する。
    let installed = catalog
        .installed_directory(&id("dev.example.terminal"))
        .unwrap();
    assert_eq!(
        std::fs::read(installed.join("extension.wasm")).unwrap(),
        b"\0asm"
    );
    assert!(
        catalog
            .data_directory(&id("dev.example.terminal"))
            .unwrap()
            .is_dir()
    );

    // 再起動しても同じ宣言を読める。
    let reopened = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    assert_eq!(reopened.entries().next().unwrap(), entry);
}

#[test]
fn reinstalling_the_same_package_does_not_rewrite_the_catalog() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let manifest = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "terminal.lapis", &manifest, files);

    let loader = loader();
    loader.install(&mut catalog, &package, true).unwrap();
    let saved = std::fs::read(workspace.path().join("extensions").join("catalog.json")).unwrap();
    loader.install(&mut catalog, &package, true).unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("extensions").join("catalog.json")).unwrap(),
        saved
    );
}

#[test]
fn an_incompatible_package_is_declared_but_not_installed() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    // Host の版より新しい契約を要求する。
    let manifest = extension_manifest(
        "dev.example.future",
        &[(EXTENSION_CONTRACT, "99.0.0", "99.9.0")],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "future.lapis", &manifest, files);

    let outcome = loader()
        .install(&mut catalog, &package, true)
        .expect("署名は正しいので読み込み自体は成功する");
    assert!(!outcome.is_installed());
    assert!(matches!(
        outcome,
        InstallOutcome::Incompatible {
            ref reason,
            kept_previous: false,
            ..
        } if matches!(reason, extensions_entry::Incompatibility::VersionOutOfRange { .. })
    ));

    // 実体は置かず、非互換として宣言する。
    let state = catalog.get(&id("dev.example.future")).unwrap().state;
    assert!(!state.is_installed());
    assert!(!state.is_enabled());
    assert!(state.is_incompatible());
    assert!(
        !catalog
            .installed_directory(&id("dev.example.future"))
            .unwrap()
            .exists(),
        "非互換の package は展開しない"
    );

    // 非互換の拡張は有効にできない。
    assert!(
        loader()
            .set_enabled(&mut catalog, &id("dev.example.future"), true)
            .unwrap()
            .is_none()
    );
    assert!(
        !catalog
            .get(&id("dev.example.future"))
            .unwrap()
            .state
            .is_enabled()
    );
}

#[test]
fn a_package_that_requires_an_unprovided_contract_is_incompatible() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let manifest = extension_manifest(
        "dev.example.wasm",
        &[("lapis.extension.wasm", "1.0.0", "1.0.0")],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "wasm.lapis", &manifest, files);

    let outcome = loader().install(&mut catalog, &package, true).unwrap();
    assert!(matches!(
        outcome,
        InstallOutcome::Incompatible {
            reason: extensions_entry::Incompatibility::UnprovidedContract { .. },
            ..
        }
    ));
    assert!(
        catalog
            .get(&id("dev.example.wasm"))
            .unwrap()
            .state
            .is_incompatible()
    );
}

#[test]
fn an_incompatible_upgrade_keeps_the_installed_version() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let loader = loader();
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let supported = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let package = write_package(workspace.path(), "supported.lapis", &supported, files);
    loader.install(&mut catalog, &package, true).unwrap();

    // 動いている版の上へ、非互換の版を入れようとする。
    let future = signed_manifest(
        "extension",
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, "99.0.0", "99.9.0")],
        files,
        |manifest| manifest["version"] = json!("9.0.0"),
    );
    let package = write_package(workspace.path(), "future.lapis", &future, files);
    let outcome = loader.install(&mut catalog, &package, true).unwrap();
    assert!(matches!(
        outcome,
        InstallOutcome::Incompatible {
            kept_previous: true,
            ..
        }
    ));

    // 動いている版の宣言と実体はそのまま残る。
    let entry = catalog.get(&id("dev.example.terminal")).unwrap();
    assert!(entry.state.is_installed());
    assert!(entry.state.is_enabled());
    assert!(!entry.state.is_incompatible());
    assert_eq!(entry.version, ExtensionVersion::parse("0.2.0").unwrap());
    assert_eq!(
        std::fs::read(
            catalog
                .installed_directory(&id("dev.example.terminal"))
                .unwrap()
                .join("extension.wasm")
        )
        .unwrap(),
        b"\0asm"
    );
}

#[test]
fn a_package_with_an_invalid_signature_is_not_installed_and_not_declared() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    // 署名の後に manifest を書き換え、宣言と署名を食い違わせる。
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let mut manifest = extension_manifest(
        "dev.example.tampered",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    manifest["version"] = json!("9.9.9");
    let package = write_package(workspace.path(), "tampered.lapis", &manifest, files);

    let error = loader()
        .install(&mut catalog, &package, true)
        .expect_err("署名が一致しない package は読み込まない");
    assert!(matches!(error, PackageError::Signature(_)), "{error:?}");
    assert_eq!(
        catalog.entries().count(),
        0,
        "署名が不正な package は id も信頼できないため目録に載せない"
    );
    assert!(
        !catalog
            .installed_directory(&id("dev.example.tampered"))
            .unwrap()
            .exists()
    );
}

#[test]
fn a_package_signed_with_an_untrusted_key_is_rejected() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let manifest = signed_manifest(
        "extension",
        "dev.example.untrusted",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
        files,
        |manifest| manifest["key_id"] = json!("not-a-trusted-key"),
    );
    let package = write_package(workspace.path(), "untrusted.lapis", &manifest, files);

    let error = loader().install(&mut catalog, &package, true).unwrap_err();
    assert!(matches!(error, PackageError::Signature(_)), "{error:?}");
    assert_eq!(catalog.entries().count(), 0);
}

#[test]
fn a_tampered_file_is_rejected_by_the_sha256_check() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let declared: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let manifest = extension_manifest(
        "dev.example.tampered_file",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    // manifest は元の bytes を宣言したまま、zip には別の内容を入れる。
    let actual: &[(&str, &[u8])] = &[("extension.wasm", b"not wasm")];
    let package = write_package(workspace.path(), "tampered-file.lapis", &manifest, actual);
    assert_ne!(declared[0].1, actual[0].1);

    let error = loader().install(&mut catalog, &package, true).unwrap_err();
    assert!(matches!(error, PackageError::Malformed(_)), "{error:?}");
    assert_eq!(catalog.entries().count(), 0);
}

#[test]
fn an_undeclared_file_is_rejected() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let manifest = extension_manifest(
        "dev.example.extra",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let with_extra: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm"), ("extra.sh", b"rm -rf")];
    let package = write_package(workspace.path(), "extra.lapis", &manifest, with_extra);
    assert_eq!(files.len(), 1);

    let error = loader().install(&mut catalog, &package, true).unwrap_err();
    assert!(matches!(error, PackageError::Malformed(_)), "{error:?}");
    assert_eq!(catalog.entries().count(), 0);
}

#[test]
fn a_host_package_kind_is_not_loaded_as_an_extension() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let files: &[(&str, &[u8])] = &[("host.json", b"{}")];
    // 種別は manifest が決める。同じ zip でも `kind` が host なら拡張として読まない。
    let manifest = signed_manifest(
        "host",
        "dev.example.host",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
        files,
        |_| {},
    );
    let package = write_package(workspace.path(), "host.lapis-host", &manifest, files);

    let error = loader().install(&mut catalog, &package, true).unwrap_err();
    assert!(matches!(
        error,
        PackageError::NotAnExtension(PackageKind::Host)
    ));
    assert_eq!(catalog.entries().count(), 0);
}

#[test]
fn a_broken_or_missing_manifest_is_rejected() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let path = workspace.path().join("broken.lapis");
    std::fs::write(&path, b"not a zip").unwrap();
    assert!(matches!(
        loader().install(&mut catalog, &path, true),
        Err(PackageError::Malformed(_))
    ));

    // manifest の無い zip。
    use std::io::Write as _;
    let empty = workspace.path().join("empty.lapis");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&empty).unwrap());
    writer
        .start_file("extension.wasm", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"\0asm").unwrap();
    writer.finish().unwrap();
    assert!(matches!(
        loader().install(&mut catalog, &empty, true),
        Err(PackageError::Malformed(_))
    ));
    assert_eq!(catalog.entries().count(), 0);
}

#[test]
fn reinstalling_a_compatible_version_clears_the_incompatible_declaration() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let loader = loader();
    let future = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, "99.0.0", "99.9.0")],
    );
    let package = write_package(workspace.path(), "future.lapis", &future, files);
    loader.install(&mut catalog, &package, true).unwrap();
    assert!(
        catalog
            .get(&id("dev.example.terminal"))
            .unwrap()
            .state
            .is_incompatible()
    );

    // Host が対応する版の package を入れ直すと、宣言は導入済みへ戻る。
    let supported = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let package = write_package(workspace.path(), "supported.lapis", &supported, files);
    let outcome = loader.install(&mut catalog, &package, true).unwrap();
    assert!(outcome.is_installed());
    let state = catalog.get(&id("dev.example.terminal")).unwrap().state;
    assert!(state.is_installed());
    assert!(state.is_enabled());
    assert!(!state.is_incompatible());
}

#[test]
fn installing_with_a_stale_catalog_keeps_other_declarations() {
    let workspace = tempfile::tempdir().unwrap();
    let mut stale = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    stale
        .register(ExtensionEntry::new(
            id("dev.example.other"),
            ExtensionVersion::parse("0.1.0").unwrap(),
            ExtensionState::installed(false),
        ))
        .unwrap();

    // 別の入口が同じ目録へ宣言を足す。
    let mut other = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    other
        .register(ExtensionEntry::new(
            id("dev.example.later"),
            ExtensionVersion::parse("0.1.0").unwrap(),
            ExtensionState::installed(true),
        ))
        .unwrap();
    drop(other);

    let manifest = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "terminal.lapis", &manifest, files);
    loader().install(&mut stale, &package, true).unwrap();

    let entries: Vec<_> = ExtensionCatalog::open(workspace.path().join("extensions"))
        .unwrap()
        .entries()
        .map(|entry| entry.id.as_str().to_owned())
        .collect();
    assert_eq!(
        entries,
        vec![
            "dev.example.later",
            "dev.example.other",
            "dev.example.terminal"
        ],
        "古い写しの読み込みで、別の入口が足した宣言を失わない"
    );
}

#[test]
fn disabling_and_enabling_is_a_declaration_change_reflected_on_restart() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let manifest = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "terminal.lapis", &manifest, files);

    let loader = loader();
    loader.install(&mut catalog, &package, true).unwrap();
    let registration = loader
        .set_enabled(&mut catalog, &id("dev.example.terminal"), false)
        .unwrap()
        .expect("導入済みなので変更できる");
    assert_eq!(registration, extensions_entry::Registration::Updated);

    // 同じ状態への再登録は file を書き換えない。
    let saved = std::fs::read(workspace.path().join("extensions").join("catalog.json")).unwrap();
    assert_eq!(
        loader
            .set_enabled(&mut catalog, &id("dev.example.terminal"), false)
            .unwrap(),
        Some(extensions_entry::Registration::Unchanged)
    );
    assert_eq!(
        std::fs::read(workspace.path().join("extensions").join("catalog.json")).unwrap(),
        saved
    );

    // 再起動（目録の開き直し）で無効が反映される。
    let reopened = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let entry = reopened.get(&id("dev.example.terminal")).unwrap();
    assert!(entry.state.is_installed());
    assert!(!entry.state.is_enabled());
    assert!(
        reopened
            .installed_directory(&id("dev.example.terminal"))
            .unwrap()
            .exists(),
        "無効化は package を取り除かない"
    );

    // 有効に戻せる。
    let mut reopened = reopened;
    assert_eq!(
        loader
            .set_enabled(&mut reopened, &id("dev.example.terminal"), true)
            .unwrap(),
        Some(extensions_entry::Registration::Updated)
    );
}

#[test]
fn a_version_upgrade_replaces_the_installed_package() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let loader = loader();
    let old: &[(&str, &[u8])] = &[("extension.wasm", b"old")];
    let manifest = signed_manifest(
        "extension",
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
        old,
        |_| {},
    );
    let package = write_package(workspace.path(), "old.lapis", &manifest, old);
    loader.install(&mut catalog, &package, true).unwrap();

    let new: &[(&str, &[u8])] = &[("extension.wasm", b"new")];
    let upgraded = signed_manifest(
        "extension",
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
        new,
        |manifest| manifest["version"] = json!("0.3.0"),
    );
    let package = write_package(workspace.path(), "new.lapis", &upgraded, new);
    loader.install(&mut catalog, &package, true).unwrap();

    let entry = catalog.get(&id("dev.example.terminal")).unwrap();
    assert_eq!(entry.version, ExtensionVersion::parse("0.3.0").unwrap());
    assert_eq!(
        std::fs::read(
            catalog
                .installed_directory(&id("dev.example.terminal"))
                .unwrap()
                .join("extension.wasm")
        )
        .unwrap(),
        b"new",
        "旧版の file を残さない"
    );
    assert_eq!(catalog.entries().count(), 1);
}

#[test]
fn a_package_with_a_trailing_dot_path_is_rejected() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let files: &[(&str, &[u8])] = &[("a.txt.", b"x")];
    let manifest = signed_manifest(
        "extension",
        "dev.example.dot",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
        files,
        |_| {},
    );
    let package = write_package(workspace.path(), "dot.lapis", &manifest, files);

    let error = loader().install(&mut catalog, &package, true).unwrap_err();
    assert!(matches!(error, PackageError::Malformed(_)), "{error:?}");
    assert_eq!(catalog.entries().count(), 0);
}

#[test]
fn a_package_with_a_duplicate_member_name_is_rejected() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let manifest = extension_manifest(
        "dev.example.duplicate",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    // `zip` の writer は同名 member を書けないため、central directory を直接組んで
    // 同名 member を持つ zip を作る。
    let path = workspace.path().join("duplicate.lapis");
    write_duplicate_member_package(&path, &manifest, files);

    let error = loader().install(&mut catalog, &path, true).unwrap_err();
    assert!(matches!(error, PackageError::Malformed(_)), "{error:?}");
    assert_eq!(catalog.entries().count(), 0);
}

/// 同名 member を 2 つ持つ zip を、最小の ZIP 構造で組み立てる。
///
/// `zip` crate の writer は同名を拒否するため、central directory と local header を
/// 手で書き、`ZipArchive::has_duplicate_names` が真になる入力を作る。
fn write_duplicate_member_package(path: &Path, manifest: &Value, files: &[(&str, &[u8])]) {
    fn local_header(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes()); // version needed
        header.extend_from_slice(&0u16.to_le_bytes()); // flags
        header.extend_from_slice(&0u16.to_le_bytes()); // stored
        header.extend_from_slice(&0u16.to_le_bytes()); // time
        header.extend_from_slice(&0u16.to_le_bytes()); // date
        header.extend_from_slice(&[0u8; 4]); // crc32 (未検証)
        header.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        header.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra len
        header.extend_from_slice(name.as_bytes());
        header.extend_from_slice(bytes);
        header
    }
    fn central_header(name: &str, bytes: &[u8], offset: u32) -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes()); // version made by
        header.extend_from_slice(&20u16.to_le_bytes()); // version needed
        header.extend_from_slice(&0u16.to_le_bytes()); // flags
        header.extend_from_slice(&0u16.to_le_bytes()); // stored
        header.extend_from_slice(&0u16.to_le_bytes()); // time
        header.extend_from_slice(&0u16.to_le_bytes()); // date
        header.extend_from_slice(&[0u8; 4]); // crc32
        header.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        header.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra
        header.extend_from_slice(&0u16.to_le_bytes()); // comment
        header.extend_from_slice(&0u16.to_le_bytes()); // disk
        header.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        header.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        header.extend_from_slice(&offset.to_le_bytes());
        header.extend_from_slice(name.as_bytes());
        header
    }

    let mut body = Vec::new();
    let mut central = Vec::new();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    entries.push((
        MANIFEST_NAME.to_owned(),
        serde_json::to_vec(manifest).unwrap(),
    ));
    for (name, bytes) in files {
        entries.push(((*name).to_owned(), bytes.to_vec()));
    }
    // 最後の file を 2 回入れる。
    let duplicated = entries.last().unwrap().clone();
    entries.push(duplicated);
    for (name, bytes) in &entries {
        let offset = body.len() as u32;
        body.extend_from_slice(&local_header(name, bytes));
        central.extend_from_slice(&central_header(name, bytes, offset));
    }
    let mut zip = body;
    let central_offset = zip.len() as u32;
    zip.extend_from_slice(&central);
    zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes()); // disk
    zip.extend_from_slice(&0u16.to_le_bytes()); // cd disk
    zip.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    zip.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    zip.extend_from_slice(&(central.len() as u32).to_le_bytes());
    zip.extend_from_slice(&central_offset.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes()); // comment len
    std::fs::write(path, zip).unwrap();
}

#[test]
fn a_failed_catalog_write_keeps_the_previous_install_intact() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let loader = loader();
    let old: &[(&str, &[u8])] = &[("extension.wasm", b"old")];
    let manifest = signed_manifest(
        "extension",
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
        old,
        |_| {},
    );
    let package = write_package(workspace.path(), "old.lapis", &manifest, old);
    loader.install(&mut catalog, &package, true).unwrap();

    // 目録の書き込みを失敗させる。データの置き場所を file で塞ぐと、登録は配置を
    // 用意できずに失敗する。
    let data = catalog.data_directory(&id("dev.example.terminal")).unwrap();
    std::fs::remove_dir_all(&data).unwrap();
    std::fs::write(&data, b"blocked").unwrap();

    let new: &[(&str, &[u8])] = &[("extension.wasm", b"new")];
    let upgraded = signed_manifest(
        "extension",
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
        new,
        |manifest| manifest["version"] = json!("0.3.0"),
    );
    let package = write_package(workspace.path(), "new.lapis", &upgraded, new);
    assert!(loader.install(&mut catalog, &package, true).is_err());

    // 動いている版の宣言と実体はそのまま。書きかけの実体も残さない。
    let entry = catalog.get(&id("dev.example.terminal")).unwrap();
    assert!(entry.state.is_installed());
    assert_eq!(entry.version, ExtensionVersion::parse("0.2.0").unwrap());
    let installed = catalog
        .installed_directory(&id("dev.example.terminal"))
        .unwrap();
    assert_eq!(
        std::fs::read(installed.join("extension.wasm")).unwrap(),
        b"old"
    );
    let staging: Vec<_> = std::fs::read_dir(workspace.path().join("extensions").join("installed"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".staging-"))
        .collect();
    assert!(staging.is_empty(), "書きかけの実体を残さない");
}

#[test]
fn an_interrupted_install_is_recovered_before_the_catalog_is_published() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    // 目録だけが「導入済み」へ進み、実体を置く前に落ちた状態を作る。
    catalog
        .register(ExtensionEntry::new(
            id("dev.example.partial"),
            ExtensionVersion::parse("0.1.0").unwrap(),
            ExtensionState::installed(true),
        ))
        .unwrap();
    let root = catalog.root().to_path_buf();
    std::fs::write(
        root.join("journal.json"),
        serde_json::to_vec(&json!({
            "schema": 1,
            "pending": [{
                "id": "dev.example.partial",
                "previous": null,
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    drop(catalog);

    // 起動時の回復が、実体の無い「導入済み」を宣言として配らない。
    let mut reopened = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    extensions_entry::recover_pending_installs(&mut reopened).unwrap();
    assert!(reopened.get(&id("dev.example.partial")).is_none());
    assert!(!root.join("journal.json").exists());
}

#[test]
fn removing_a_package_keeps_the_files_when_the_catalog_write_fails() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let manifest = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "terminal.lapis", &manifest, files);
    let loader = loader();
    loader.install(&mut catalog, &package, true).unwrap();

    // 目録の書き込みを失敗させる。`data` を file で塞ぐと配置の用意に失敗する。
    let data = catalog.data_directory(&id("dev.example.terminal")).unwrap();
    std::fs::remove_dir_all(&data).unwrap();
    std::fs::write(&data, b"blocked").unwrap();

    assert!(
        loader
            .remove(&mut catalog, &id("dev.example.terminal"))
            .is_err()
    );
    // 実体を失わない。
    let installed = catalog
        .installed_directory(&id("dev.example.terminal"))
        .unwrap();
    assert_eq!(
        std::fs::read(installed.join("extension.wasm")).unwrap(),
        b"\0asm"
    );
    assert!(
        catalog
            .get(&id("dev.example.terminal"))
            .unwrap()
            .state
            .is_installed()
    );
}

#[test]
fn removing_a_package_keeps_the_extension_data() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let manifest = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "terminal.lapis", &manifest, files);
    let loader = loader();
    loader.install(&mut catalog, &package, true).unwrap();
    std::fs::write(
        catalog
            .data_directory(&id("dev.example.terminal"))
            .unwrap()
            .join("settings.json"),
        b"{}",
    )
    .unwrap();

    loader
        .remove(&mut catalog, &id("dev.example.terminal"))
        .unwrap();
    let entry = catalog.get(&id("dev.example.terminal")).unwrap();
    assert!(!entry.state.is_installed());
    assert!(!entry.state.is_enabled());
    assert!(
        !catalog
            .installed_directory(&id("dev.example.terminal"))
            .unwrap()
            .exists()
    );
    assert!(
        catalog
            .data_directory(&id("dev.example.terminal"))
            .unwrap()
            .join("settings.json")
            .is_file(),
        "拡張データは再導入で引き継ぐため残す"
    );

    // 目録に無い ID は取り除けない。
    assert!(
        loader
            .remove(&mut catalog, &id("dev.example.none"))
            .is_err()
    );
}

#[test]
fn a_catalog_written_before_the_incompatible_state_is_readable() {
    let workspace = tempfile::tempdir().unwrap();
    let directory = workspace.path().join("extensions");
    std::fs::create_dir_all(&directory).unwrap();
    // `incompatible` を導入する前の目録。省略時は false として読む。
    std::fs::write(
        directory.join("catalog.json"),
        r#"{"schema":1,"extensions":[{"id":"dev.example.old","version":"0.1.0","installed":true,"enabled":false}]}"#,
    )
    .unwrap();
    let catalog = ExtensionCatalog::open(directory).unwrap();
    let state = catalog.get(&id("dev.example.old")).unwrap().state;
    assert!(state.is_installed());
    assert!(!state.is_enabled());
    assert!(!state.is_incompatible());
}

#[test]
fn contract_ids_and_ranges_are_validated() {
    assert!(ContractId::try_new(EXTENSION_CONTRACT).is_ok());
    assert!(ContractId::try_new("Lapis.Extension").is_err());
    // 範囲の逆転は manifest の読み込みで拒否する。
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let manifest = extension_manifest(
        "dev.example.reversed",
        &[("lapis.extension", "2.0.0", "1.0.0")],
    );
    let package = write_package(workspace.path(), "reversed.lapis", &manifest, files);
    assert!(matches!(
        loader().install(&mut catalog, &package, true),
        Err(PackageError::Malformed(_))
    ));
}

#[test]
fn declarations_are_aggregated_from_the_catalog() {
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = ExtensionCatalog::open(workspace.path().join("extensions")).unwrap();
    let manifest = extension_manifest(
        "dev.example.terminal",
        &[(EXTENSION_CONTRACT, &host_version(), &host_version())],
    );
    let files: &[(&str, &[u8])] = &[("extension.wasm", b"\0asm")];
    let package = write_package(workspace.path(), "terminal.lapis", &manifest, files);
    loader().install(&mut catalog, &package, true).unwrap();
    catalog
        .register(ExtensionEntry::new(
            id("dev.example.pending"),
            ExtensionVersion::parse("0.1.0").unwrap(),
            ExtensionState::incompatible(),
        ))
        .unwrap();

    let declarations = extensions_entry::ExtensionDeclarations::from_catalog(&catalog);
    assert_eq!(
        declarations
            .extensions
            .iter()
            .map(|declaration| (
                declaration.id.as_str(),
                declaration.installed,
                declaration.enabled,
                declaration.incompatible
            ))
            .collect::<Vec<_>>(),
        vec![
            ("dev.example.pending", false, false, true),
            ("dev.example.terminal", true, true, false),
        ]
    );
    assert_eq!(
        declarations
            .loaded()
            .map(|declaration| declaration.id.as_str())
            .collect::<Vec<_>>(),
        vec!["dev.example.terminal"]
    );
}
