//! 導入・除去の途中経過を記録し、中断した操作を起動時に回復する。
//!
//! 目録（宣言）と `installed/<id>/`（実体）は別の file なので、2 つの書き込みの間で
//! Host が終了すると食い違う。ここでは操作の開始時に「何をどこまで進めたか」と
//! 「操作前の宣言」を journal へ記録し、完了時に消す。起動時に [`recover`] が残った
//! 記録を見て、段階に応じて完了または取り消しを行う。
//!
//! 段階を持つのは、中断した位置によって戻すべき状態が違うため。
//!
//! - `Prepared`: 目録も実体も変えていない。記録を消すだけ。
//! - `CatalogWritten`: 目録だけ新しい。実体は旧版のまま（または無い）。宣言を取り消す。
//! - `BackedUp`: 旧実体を `.swap/<id>` へ退避した。実体を戻してから宣言を取り消す。
//! - `Swapped`: 新しい実体を置いた（除去なら実体を退避し終えた）。そのまま完了する。
//!
//! 回復は導入の lock を取得して行う。別の Host が導入中の操作を中断済みと誤認しない。
//! 同じ段階からの回復は冪等であり、回復中に再度落ちても次の起動で同じ結果になる。

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
/// 導入の排他 file。
const LOCK_FILE: &str = "installed.lock";

/// 目録が返す配置解決の失敗は、そのまま目録の誤りとして扱う。
fn resolve<T>(
    result: Result<T, crate::InvalidExtensionPackageId>,
) -> Result<T, ExtensionCatalogError> {
    result.map_err(|_| ExtensionCatalogError::InvalidEntry)
}

/// 進行中の操作の種類。回復時に「実体を戻す」か「実体を捨てる」かを決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingOperation {
    Install,
    Remove,
}

/// 操作の進み具合。回復の判断はこの段階だけで決まる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingPhase {
    /// 目録も実体も変えていない。
    Prepared,
    /// 目録を書いた。実体はまだ置き換えていない。
    CatalogWritten,
    /// 旧実体を `.swap/<id>` へ退避した。新しい実体はまだ置いていない。
    BackedUp,
    /// 新しい実体を置いた（除去なら実体を退避し終えた）。後片付けだけが残る。
    Swapped,
}

/// 1 件の未完了操作。`previous` は操作前の宣言（無ければ新規導入）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInstall {
    pub id: ExtensionPackageId,
    pub operation: PendingOperation,
    pub phase: PendingPhase,
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

/// 導入の排他。同じ state directory を複数の入口が使っても、導入・回復を直列化する。
pub(crate) struct InstallLock(fs::File);

impl InstallLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self, ExtensionCatalogError> {
        fs::create_dir_all(root).map_err(ExtensionCatalogError::Io)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(LOCK_FILE))
            .map_err(ExtensionCatalogError::Io)?;
        file.lock().map_err(ExtensionCatalogError::Io)?;
        Ok(Self(file))
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// 操作の開始を記録する。同じ `id` の記録は置き換える。
pub(crate) fn begin(
    root: &Path,
    id: &ExtensionPackageId,
    operation: PendingOperation,
    previous: Option<ExtensionEntry>,
) -> Result<(), ExtensionCatalogError> {
    let mut pending = load(root)?;
    pending.insert(
        id.clone(),
        PendingInstall {
            id: id.clone(),
            operation,
            phase: PendingPhase::Prepared,
            previous,
        },
    );
    store(root, &pending)
}

/// 操作の段階を進める。記録が無ければ何もしない（完了済み）。
pub(crate) fn advance(
    root: &Path,
    id: &ExtensionPackageId,
    phase: PendingPhase,
) -> Result<(), ExtensionCatalogError> {
    let mut pending = load(root)?;
    let Some(record) = pending.get_mut(id) else {
        return Ok(());
    };
    // 段階は戻さない。呼び出し順の取り違えで回復の判断を揺らさないため。
    if phase > record.phase {
        record.phase = phase;
    }
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

/// 中断した操作を回復する。導入の lock を取得してから行う。
pub(crate) fn recover(catalog: &mut ExtensionCatalog) -> Result<usize, ExtensionCatalogError> {
    let _lock = InstallLock::acquire(catalog.root())?;
    recover_locked(catalog)
}

/// lock を取得済みの呼び出し元向けの回復。
pub(crate) fn recover_locked(
    catalog: &mut ExtensionCatalog,
) -> Result<usize, ExtensionCatalogError> {
    let root = catalog.root().to_path_buf();
    let pending = load(&root)?;
    let recovered = pending.len();
    for (id, record) in pending {
        apply(catalog, &id, &record)?;
    }
    // 記録の無い残骸（journal を書く前後で落ちた場合）も片付ける。
    clean_leftovers(catalog)?;
    // 目録の写しを file と揃える。
    catalog.reload()?;
    // 回復し終えた記録を残さない。次の起動で同じ操作を取り消さないため。
    store(&root, &BTreeMap::new())?;
    Ok(recovered)
}

fn apply(
    catalog: &mut ExtensionCatalog,
    id: &ExtensionPackageId,
    record: &PendingInstall,
) -> Result<(), ExtensionCatalogError> {
    let target = resolve(catalog.installed_directory(id))?;
    let backup = swap_path(catalog.root(), id);
    match record.phase {
        // `swapped` より前は、すべて「操作前へ戻す」で揃える。
        //
        // 段階の記録と実際の書き込みの間で落ちると、記録は実際より 1 段階前になる
        // （例: 目録を書いた直後、`catalog_written` を記録する前に落ちる）。段階ごとに
        // 違う処理をすると、その窓で取り残す。どの段階からでも同じ rollback を行えば、
        // 記録が実際より遅れていても操作前の状態に戻せる。
        PendingPhase::Prepared | PendingPhase::CatalogWritten | PendingPhase::BackedUp => {
            // 退避した旧実体があれば戻す。`target` に新しい実体の一部が残っていても、
            // 旧版で置き換える。
            if backup.is_dir() {
                if target.is_dir() {
                    fs::remove_dir_all(&target).map_err(ExtensionCatalogError::Io)?;
                }
                move_directory(&backup, &target)?;
            }
            restore_declaration(catalog, id, record.previous.clone())?;
        }
        PendingPhase::Swapped => {
            // 実体と宣言は確定している。退避した旧版だけを片付ける。除去も同じ段階を
            // 使うため、ここで宣言を触らない（既に「未導入」で書かれている）。
            if backup.is_dir() {
                fs::remove_dir_all(&backup).map_err(ExtensionCatalogError::Io)?;
            }
        }
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
                fs::remove_dir_all(&backup).map_err(ExtensionCatalogError::Io)?;
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
                fs::remove_dir_all(entry.path()).map_err(ExtensionCatalogError::Io)?;
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

    fn begin_install(root: &Path, name: &str, previous: Option<ExtensionEntry>) {
        begin(root, &id(name), PendingOperation::Install, previous).unwrap();
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
        begin_install(root.path(), "a", None);
        assert!(journal_path(root.path()).is_file());
        finish(root.path(), &id("a")).unwrap();
        assert!(!journal_path(root.path()).exists());
        assert!(
            finish(root.path(), &id("a")).is_ok(),
            "無い記録の完了は失敗しない"
        );
    }

    #[test]
    fn phases_only_move_forward() {
        let root = tempfile::tempdir().unwrap();
        begin_install(root.path(), "a", None);
        advance(root.path(), &id("a"), PendingPhase::BackedUp).unwrap();
        // 古い段階への後退は無視する。
        advance(root.path(), &id("a"), PendingPhase::Prepared).unwrap();
        assert_eq!(
            load(root.path()).unwrap()[&id("a")].phase,
            PendingPhase::BackedUp
        );
        // 記録の無い操作の段階更新は失敗しない。
        assert!(advance(root.path(), &id("b"), PendingPhase::Swapped).is_ok());
    }

    #[test]
    fn a_crash_between_the_first_install_write_and_its_phase_record_is_rolled_back() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        // 初回導入で目録を書いた直後、段階を進める前に落ちた状態。
        // 実体はまだ無く、記録は `prepared` のままである。
        catalog.register(entry("a", true)).unwrap();
        begin_install(catalog.root(), "a", None);

        recover(&mut catalog).unwrap();
        assert!(catalog.get(&id("a")).is_none(), "実体の無い宣言を残さない");
        assert!(!journal_path(catalog.root()).exists());
    }

    #[test]
    fn a_crash_between_the_write_and_its_phase_record_still_rolls_back() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog.register(entry("a", true)).unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"old").unwrap();

        // 目録を書いた直後、段階を `catalog_written` へ進める前に落ちた状態。
        // 記録は `prepared` のままである。
        catalog
            .register(ExtensionEntry::new(
                id("a"),
                ExtensionVersion::parse("0.2.0").unwrap(),
                ExtensionState::installed(true),
            ))
            .unwrap();
        begin_install(catalog.root(), "a", Some(entry("a", true)));

        recover(&mut catalog).unwrap();
        let restored = catalog.get(&id("a")).unwrap();
        assert_eq!(restored.version, ExtensionVersion::parse("0.1.0").unwrap());
        assert!(restored.state.is_installed());
        assert_eq!(
            fs::read(catalog.installed_directory(&id("a")).unwrap().join("f")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn a_crash_between_the_backup_and_its_phase_record_still_rolls_back() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog.register(entry("a", true)).unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"old").unwrap();

        // 退避した直後、段階を `backed_up` へ進める前に落ちた状態。
        begin_install(catalog.root(), "a", Some(entry("a", true)));
        advance(catalog.root(), &id("a"), PendingPhase::CatalogWritten).unwrap();
        let backup = swap_path(catalog.root(), &id("a"));
        fs::create_dir_all(backup.parent().unwrap()).unwrap();
        fs::rename(&installed, &backup).unwrap();
        // 段階は `catalog_written` のまま。

        recover(&mut catalog).unwrap();
        assert!(catalog.get(&id("a")).unwrap().state.is_installed());
        assert_eq!(
            fs::read(catalog.installed_directory(&id("a")).unwrap().join("f")).unwrap(),
            b"old"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn a_journal_at_prepared_does_not_touch_a_consistent_install() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog.register(entry("a", true)).unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"old").unwrap();
        // 何も書き換えていない状態で落ちた（`begin` の直後）。
        begin_install(catalog.root(), "a", Some(entry("a", true)));

        recover(&mut catalog).unwrap();
        assert!(catalog.get(&id("a")).unwrap().state.is_installed());
        assert_eq!(
            fs::read(catalog.installed_directory(&id("a")).unwrap().join("f")).unwrap(),
            b"old"
        );
        assert!(!journal_path(catalog.root()).exists());
    }

    #[test]
    fn an_interrupted_first_install_is_rolled_back_to_no_declaration() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        // 目録だけが先へ進み、実体が無い状態で落ちた。
        catalog.register(entry("a", true)).unwrap();
        begin_install(catalog.root(), "a", None);
        advance(catalog.root(), &id("a"), PendingPhase::CatalogWritten).unwrap();

        assert_eq!(recover(&mut catalog).unwrap(), 1);
        assert!(catalog.get(&id("a")).is_none(), "実体の無い宣言は残さない");
        assert!(!journal_path(catalog.root()).exists());
    }

    #[test]
    fn an_interruption_after_the_catalog_write_restores_the_old_declaration() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog.register(entry("a", true)).unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"old").unwrap();

        // 新版を目録へ書いたが、旧版の退避はまだ。旧実体は installed/ に残っている。
        catalog
            .register(ExtensionEntry::new(
                id("a"),
                ExtensionVersion::parse("0.2.0").unwrap(),
                ExtensionState::installed(true),
            ))
            .unwrap();
        begin_install(catalog.root(), "a", Some(entry("a", true)));
        advance(catalog.root(), &id("a"), PendingPhase::CatalogWritten).unwrap();

        recover(&mut catalog).unwrap();
        let restored = catalog.get(&id("a")).unwrap();
        assert_eq!(restored.version, ExtensionVersion::parse("0.1.0").unwrap());
        assert!(restored.state.is_installed());
        assert_eq!(
            fs::read(catalog.installed_directory(&id("a")).unwrap().join("f")).unwrap(),
            b"old"
        );
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
        begin_install(catalog.root(), "a", Some(entry("a", true)));
        advance(catalog.root(), &id("a"), PendingPhase::CatalogWritten).unwrap();
        let backup = swap_path(catalog.root(), &id("a"));
        fs::create_dir_all(backup.parent().unwrap()).unwrap();
        fs::rename(&installed, &backup).unwrap();
        advance(catalog.root(), &id("a"), PendingPhase::BackedUp).unwrap();

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
    fn a_finished_swap_keeps_the_new_version_and_cleans_the_backup() {
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
        begin_install(catalog.root(), "a", Some(entry("a", true)));
        advance(catalog.root(), &id("a"), PendingPhase::Swapped).unwrap();
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
    fn an_interrupted_remove_restores_the_files_and_declaration() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog.register(entry("a", true)).unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"old").unwrap();

        // 実体を退避したが、目録はまだ「未導入」へ変えていない。
        let backup = swap_path(catalog.root(), &id("a"));
        fs::create_dir_all(backup.parent().unwrap()).unwrap();
        fs::rename(&installed, &backup).unwrap();
        begin(
            catalog.root(),
            &id("a"),
            PendingOperation::Remove,
            Some(entry("a", true)),
        )
        .unwrap();
        advance(catalog.root(), &id("a"), PendingPhase::BackedUp).unwrap();

        recover(&mut catalog).unwrap();
        assert!(catalog.get(&id("a")).unwrap().state.is_installed());
        assert_eq!(
            fs::read(catalog.installed_directory(&id("a")).unwrap().join("f")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn recovery_is_idempotent_when_interrupted_again() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog.register(entry("a", true)).unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"old").unwrap();
        begin_install(catalog.root(), "a", Some(entry("a", true)));
        advance(catalog.root(), &id("a"), PendingPhase::BackedUp).unwrap();

        // 1 回目の回復で落ちた状況を、同じ段階のまま 2 回回復して確かめる。
        recover(&mut catalog).unwrap();
        store(
            catalog.root(),
            &BTreeMap::from([(
                id("a"),
                PendingInstall {
                    id: id("a"),
                    operation: PendingOperation::Install,
                    phase: PendingPhase::BackedUp,
                    previous: Some(entry("a", true)),
                },
            )]),
        )
        .unwrap();
        recover(&mut catalog).unwrap();
        assert_eq!(
            fs::read(catalog.installed_directory(&id("a")).unwrap().join("f")).unwrap(),
            b"old"
        );
        assert!(catalog.get(&id("a")).unwrap().state.is_installed());
    }

    #[test]
    fn concurrent_recovery_leaves_a_consistent_state() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        catalog.register(entry("a", true)).unwrap();
        let installed = catalog.installed_directory(&id("a")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("f"), b"old").unwrap();
        begin_install(catalog.root(), "a", Some(entry("a", true)));
        advance(catalog.root(), &id("a"), PendingPhase::BackedUp).unwrap();

        // 2 つの入口が同時に起動しても、lock で直列化され同じ結果になる。
        let directory = catalog.root().to_path_buf();
        drop(catalog);
        let first = std::thread::spawn({
            let directory = directory.clone();
            move || {
                let mut catalog = ExtensionCatalog::open(directory).unwrap();
                recover(&mut catalog).map(|_| ())
            }
        });
        let second = std::thread::spawn({
            let directory = directory.clone();
            move || {
                let mut catalog = ExtensionCatalog::open(directory).unwrap();
                recover(&mut catalog).map(|_| ())
            }
        });
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        let reopened = ExtensionCatalog::open(directory).unwrap();
        let restored = reopened.get(&id("a")).unwrap();
        assert!(restored.state.is_installed());
        assert_eq!(restored.version, ExtensionVersion::parse("0.1.0").unwrap());
    }

    #[test]
    fn an_unreadable_journal_is_reported_instead_of_ignored() {
        let root = tempfile::tempdir().unwrap();
        let mut catalog = catalog(root.path());
        fs::create_dir_all(catalog.root()).unwrap();
        fs::write(
            catalog.root().join(JOURNAL_FILE),
            br#"{"schema":99,"pending":[]}"#,
        )
        .unwrap();
        assert!(matches!(
            recover(&mut catalog),
            Err(ExtensionCatalogError::UnsupportedSchema(99))
        ));

        fs::write(catalog.root().join(JOURNAL_FILE), b"not json").unwrap();
        assert!(matches!(
            recover(&mut catalog),
            Err(ExtensionCatalogError::Invalid(_))
        ));
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
