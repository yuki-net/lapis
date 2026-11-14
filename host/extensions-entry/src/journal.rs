//! 導入・除去の途中経過を記録し、中断した操作を起動時に回復する。
//!
//! 目録（宣言）と `installed/<id>/`（実体）は別の file なので、2 つの書き込みの間で
//! Host が終了すると食い違う。ここでは操作の開始時に「前の宣言」を journal へ記録し、
//! 完了時に消す。起動時に [`recover`] が残った記録と実体を見て、完了または取り消しを
//! 行う。これにより「宣言は導入済みだが実体が無い」「実体は旧版だが宣言は新版」の
//! ような状態を次の起動で残さない。

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{ExtensionCatalog, ExtensionCatalogError, ExtensionEntry, ExtensionPackageId};

/// journal file の schema。項目を増やすときは上げ、旧 schema は回復せず失敗させる。
pub const JOURNAL_SCHEMA: u32 = 1;

const JOURNAL_FILE: &str = "journal.json";
/// 置き換え中の旧版を退避する directory。拡張 ID の namespace の外に置く。
pub(crate) const SWAP_DIR: &str = ".swap";
/// 展開中の一時 directory の接頭辞。
const STAGING_PREFIX: &str = ".staging-";

/// 目録が返す配置解決の失敗は、そのまま目録の誤りとして扱う。
fn resolve<T>(
    result: Result<T, crate::InvalidExtensionPackageId>,
) -> Result<T, ExtensionCatalogError> {
    result.map_err(|_| ExtensionCatalogError::InvalidEntry)
}

/// 1 件の未完了操作。`previous` は操作前の宣言（無ければ新規導入）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInstall {
    pub id: ExtensionPackageId,
    /// 操作前の宣言。取り消すときに戻す。`None` は目録に無かった（新規）。
    #[serde(default)]
    pub previous: Option<ExtensionEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalDocument {
    schema: u32,
    pending: Vec<PendingInstall>,
}

/// 操作の開始を記録する。同じ `id` の記録は置き換える。
pub(crate) fn begin(
    root: &Path,
    id: &ExtensionPackageId,
    previous: Option<ExtensionEntry>,
) -> Result<(), ExtensionCatalogError> {
    let mut pending = load(root)?;
    pending.insert(
        id.clone(),
        PendingInstall {
            id: id.clone(),
            previous,
        },
    );
    store(root, &pending)
}

/// 操作の完了を記録する。記録が無ければ何もしない。
pub(crate) fn finish(root: &Path, id: &ExtensionPackageId) -> Result<(), ExtensionCatalogError> {
    let mut pending = load(root)?;
    if pending.remove(id).is_none() {
        return Ok(());
    }
    store(root, &pending)
}

/// 中断した操作を回復する。journal に記録の無い宣言には触れない。
pub(crate) fn recover(catalog: &mut ExtensionCatalog) -> Result<usize, ExtensionCatalogError> {
    let root = catalog.root().to_path_buf();
    let pending = load(&root)?;
    let mut recovered = 0;
    for (id, record) in pending {
        recover_one(catalog, &id, record.previous)?;
        recovered += 1;
    }
    // 記録の無い残骸（journal を書く前後で落ちた場合）も片付ける。
    clean_leftovers(catalog)?;
    // 目録の写しを file と揃える。
    catalog.reload()?;
    // 回復し終えた記録を残さない。次の起動で同じ操作を取り消さないため。
    store(&root, &BTreeMap::new())?;
    Ok(recovered)
}

fn recover_one(
    catalog: &mut ExtensionCatalog,
    id: &ExtensionPackageId,
    previous: Option<ExtensionEntry>,
) -> Result<(), ExtensionCatalogError> {
    let target = resolve(catalog.installed_directory(id))?;
    let backup = swap_path(catalog.root(), id);
    if backup.is_dir() {
        if target.is_dir() {
            // 新しい実体の配置まで終わっている。退避は不要。
            let _ = fs::remove_dir_all(&backup);
        } else {
            // 旧版を退避した後、新しい実体を置けずに落ちた。旧版へ戻し、
            // 宣言も操作前へ戻す。
            move_directory(&backup, &target)?;
            restore_declaration(catalog, id, previous)?;
        }
        return Ok(());
    }
    // 退避は無い。実体が無ければ初回導入の途中で落ちている。
    if !target.is_dir() {
        restore_declaration(catalog, id, previous)?;
    }
    Ok(())
}

fn restore_declaration(
    catalog: &mut ExtensionCatalog,
    id: &ExtensionPackageId,
    previous: Option<ExtensionEntry>,
) -> Result<(), ExtensionCatalogError> {
    let target = resolve(catalog.installed_directory(id))?;
    match previous {
        // 実体があるときだけ「導入済み」へ戻す。無ければ正直に未導入とする。
        Some(entry) if entry.state.is_installed() && target.is_dir() => {
            catalog.register(entry)?;
        }
        Some(entry) => {
            catalog.register(ExtensionEntry::new(
                entry.id,
                entry.version,
                crate::ExtensionState::unavailable(),
            ))?;
        }
        None => catalog.forget(id)?,
    }
    Ok(())
}

/// journal に載っていない `.swap` と `.staging-*` を片付ける。
fn clean_leftovers(catalog: &mut ExtensionCatalog) -> Result<(), ExtensionCatalogError> {
    let root = catalog.root().to_path_buf();
    let swap = root.join(SWAP_DIR);
    if swap.is_dir() {
        for entry in fs::read_dir(&swap).map_err(ExtensionCatalogError::Io)? {
            let entry = entry.map_err(ExtensionCatalogError::Io)?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| ExtensionPackageId::try_new(name.to_owned()).ok())
            else {
                continue;
            };
            let backup = swap_path(&root, &id);
            let target = resolve(catalog.installed_directory(&id))?;
            if target.is_dir() {
                let _ = fs::remove_dir_all(&backup);
            } else {
                move_directory(&backup, &target)?;
            }
        }
    }
    let installed = root.join("installed");
    if installed.is_dir() {
        for entry in fs::read_dir(&installed).map_err(ExtensionCatalogError::Io)? {
            let entry = entry.map_err(ExtensionCatalogError::Io)?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(())
}

pub(crate) fn swap_path(root: &Path, id: &ExtensionPackageId) -> PathBuf {
    root.join(SWAP_DIR).join(id.as_str())
}

fn move_directory(from: &Path, to: &Path) -> Result<(), ExtensionCatalogError> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(ExtensionCatalogError::Io)?;
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_directory(from, to)?;
            fs::remove_dir_all(from).map_err(ExtensionCatalogError::Io)
        }
        Err(error) => Err(ExtensionCatalogError::Io(error)),
    }
}

fn copy_directory(from: &Path, to: &Path) -> Result<(), ExtensionCatalogError> {
    fs::create_dir_all(to).map_err(ExtensionCatalogError::Io)?;
    for entry in fs::read_dir(from).map_err(ExtensionCatalogError::Io)? {
        let entry = entry.map_err(ExtensionCatalogError::Io)?;
        let path = entry.path();
        let destination = to.join(entry.file_name());
        if path.is_dir() {
            copy_directory(&path, &destination)?;
        } else {
            fs::copy(&path, &destination).map_err(ExtensionCatalogError::Io)?;
        }
    }
    Ok(())
}

fn journal_path(root: &Path) -> PathBuf {
    root.join(JOURNAL_FILE)
}

fn load(
    root: &Path,
) -> Result<BTreeMap<ExtensionPackageId, PendingInstall>, ExtensionCatalogError> {
    let bytes = match fs::read(journal_path(root)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(ExtensionCatalogError::Io(error)),
    };
    let document: JournalDocument =
        serde_json::from_slice(&bytes).map_err(ExtensionCatalogError::Invalid)?;
    if document.schema != JOURNAL_SCHEMA {
        return Err(ExtensionCatalogError::UnsupportedSchema(document.schema));
    }
    let mut pending = BTreeMap::new();
    for record in document.pending {
        if pending.insert(record.id.clone(), record).is_some() {
            return Err(ExtensionCatalogError::InvalidEntry);
        }
    }
    Ok(pending)
}

fn store(
    root: &Path,
    pending: &BTreeMap<ExtensionPackageId, PendingInstall>,
) -> Result<(), ExtensionCatalogError> {
    fs::create_dir_all(root).map_err(ExtensionCatalogError::Io)?;
    if pending.is_empty() {
        match fs::remove_file(journal_path(root)) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(ExtensionCatalogError::Io(error)),
        }
    }
    let document = JournalDocument {
        schema: JOURNAL_SCHEMA,
        pending: pending.values().cloned().collect(),
    };
    let bytes = serde_json::to_vec_pretty(&document).map_err(ExtensionCatalogError::Invalid)?;
    let mut temporary = tempfile::NamedTempFile::new_in(root).map_err(ExtensionCatalogError::Io)?;
    {
        use io::Write;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(ExtensionCatalogError::Io)?;
    }
    temporary
        .persist(journal_path(root))
        .map_err(|error| ExtensionCatalogError::Io(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExtensionState, ExtensionVersion};

    fn id(value: &str) -> ExtensionPackageId {
        ExtensionPackageId::try_new(value).unwrap()
    }

    fn entry(value: &str, enabled: bool) -> ExtensionEntry {
        ExtensionEntry::new(
            id(value),
            ExtensionVersion::parse("0.1.0").unwrap(),
            ExtensionState::installed(enabled),
        )
    }

    fn catalog(root: &Path) -> ExtensionCatalog {
        ExtensionCatalog::open(root.join("extensions")).unwrap()
    }

    #[test]
    fn a_missing_journal_is_empty_and_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(load(root.path()).unwrap().is_empty());
        assert!(!journal_path(root.path()).exists());
    }

    #[test]
    fn a_finished_operation_removes_the_journal() {
        let root = tempfile::tempdir().unwrap();
        begin(root.path(), &id("a"), None).unwrap();
        assert!(journal_path(root.path()).is_file());
        finish(root.path(), &id("a")).unwrap();
        assert!(!journal_path(root.path()).exists());
        assert!(
            finish(root.path(), &id("a")).is_ok(),
            "無い記録の完了は失敗しない"
        );
    }

    #[test]
    fn an_interrupted_first_install_is_rolled_back_to_no_declaration() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        // 目録だけが先へ進み、実体が無い状態で落ちた。
        catalog.register(entry("a", true)).unwrap();
        begin(catalog.root(), &id("a"), None).unwrap();

        assert_eq!(recover(&mut catalog).unwrap(), 1);
        assert!(catalog.get(&id("a")).is_none(), "実体の無い宣言は残さない");
        assert!(!journal_path(catalog.root()).exists());
    }

    #[test]
    fn a_declaration_without_a_journal_entry_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        // 宣言だけを登録する段階（PR1 の配置解決）を、回復が書き換えない。
        catalog.register(entry("a", true)).unwrap();
        assert_eq!(recover(&mut catalog).unwrap(), 0);
        assert!(catalog.get(&id("a")).unwrap().state.is_installed());
    }

    #[test]
    fn an_interrupted_upgrade_restores_the_previous_version() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog.register(entry("a", true)).unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"old").unwrap();

        // 新版を目録へ書き、旧版を退避した後で落ちた。
        catalog
            .register(ExtensionEntry::new(
                id("a"),
                ExtensionVersion::parse("0.2.0").unwrap(),
                ExtensionState::installed(true),
            ))
            .unwrap();
        begin(catalog.root(), &id("a"), Some(entry("a", true))).unwrap();
        let backup = swap_path(catalog.root(), &id("a"));
        fs::create_dir_all(backup.parent().unwrap()).unwrap();
        fs::rename(&installed, &backup).unwrap();

        recover(&mut catalog).unwrap();
        let restored = catalog.get(&id("a")).unwrap();
        assert_eq!(restored.version, ExtensionVersion::parse("0.1.0").unwrap());
        assert!(restored.state.is_installed());
        assert_eq!(
            fs::read(catalog.installed_directory(&id("a")).unwrap().join("f")).unwrap(),
            b"old"
        );
        assert!(!backup.exists(), "退避先を残さない");
    }

    #[test]
    fn a_finished_swap_only_cleans_the_backup() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog
            .register(ExtensionEntry::new(
                id("a"),
                ExtensionVersion::parse("0.2.0").unwrap(),
                ExtensionState::installed(true),
            ))
            .unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"new").unwrap();
        begin(catalog.root(), &id("a"), Some(entry("a", true))).unwrap();
        let backup = swap_path(catalog.root(), &id("a"));
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("f"), b"old").unwrap();

        recover(&mut catalog).unwrap();
        assert_eq!(
            catalog.get(&id("a")).unwrap().version,
            ExtensionVersion::parse("0.2.0").unwrap()
        );
        assert_eq!(
            fs::read(catalog.installed_directory(&id("a")).unwrap().join("f")).unwrap(),
            b"new"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn stray_staging_directories_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        let installed = catalog.root().join("installed");
        let staging = installed.join(".staging-leftover");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("f"), b"x").unwrap();

        recover(&mut catalog).unwrap();
        assert!(!staging.exists());
    }
}
