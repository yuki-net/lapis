use std::{
    collections::BTreeMap,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{ExtensionPackageId, ExtensionState, ExtensionVersion, InvalidExtensionPackageId};

/// 目録 file の schema。項目を増やすときは schema を上げ、旧 schema は受理しない。
pub const CATALOG_SCHEMA: u32 = 1;

const CATALOG_FILE: &str = "catalog.json";
const LOCK_FILE: &str = "catalog.json.lock";
const INSTALLED_DIR: &str = "installed";
const DATA_DIR: &str = "data";

/// 目録に載る 1 件。package の実体とデータの置き場所は `id` から決まる。
///
/// 保存形式は `id` / `version` に `installed` / `enabled` を並べた平坦な object と
/// する。読むときに未知の field と `enabled` かつ未導入の組み合わせを拒否する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExtensionEntry {
    pub id: ExtensionPackageId,
    pub version: ExtensionVersion,
    #[serde(flatten)]
    pub state: ExtensionState,
}

impl ExtensionEntry {
    pub fn new(id: ExtensionPackageId, version: ExtensionVersion, state: ExtensionState) -> Self {
        Self { id, version, state }
    }
}

impl<'de> Deserialize<'de> for ExtensionEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Repr {
            id: ExtensionPackageId,
            version: ExtensionVersion,
            installed: bool,
            enabled: bool,
        }

        let repr = Repr::deserialize(deserializer)?;
        let state = ExtensionState::from_parts(repr.installed, repr.enabled)
            .ok_or_else(|| serde::de::Error::custom("未導入の拡張を有効にはできません"))?;
        Ok(Self {
            id: repr.id,
            version: repr.version,
            state,
        })
    }
}

/// `catalog.json` の保存形式。`extensions` は `id` の順に並べる。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDocument {
    pub schema: u32,
    pub extensions: Vec<ExtensionEntry>,
}

/// 登録の結果。同じ宣言の再登録は `Unchanged` になり、file を書き換えない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registration {
    Created,
    Updated,
    Unchanged,
}

/// 目録の読み書きと、package・データの置き場所の解決。
///
/// ```text
/// <host state>/extensions/
///   catalog.json            目録（宣言の正本）
///   installed/<id>/         導入済み package の展開先（読み込みは別段階）
///   data/<id>/              拡張データの正本
/// ```
pub struct ExtensionCatalog {
    root: PathBuf,
    entries: BTreeMap<ExtensionPackageId, ExtensionEntry>,
}

impl ExtensionCatalog {
    /// `root` の目録を読む。file が無ければ空の目録として扱う。
    ///
    /// 壊れた file は空として扱わずに失敗させる。読み直せない目録を黙って
    /// 作り直すと、導入済み拡張の宣言を失うため。
    pub fn open(root: PathBuf) -> Result<Self, ExtensionCatalogError> {
        let entries = load_entries(&root)?;
        Ok(Self { root, entries })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entries(&self) -> impl Iterator<Item = &ExtensionEntry> {
        self.entries.values()
    }

    pub fn get(&self, id: &ExtensionPackageId) -> Option<&ExtensionEntry> {
        self.entries.get(id)
    }

    /// 宣言を登録する。同じ `id`・`version`・状態の再登録は目録を変えない。
    ///
    /// 無効化や版の交代は目録の置き換えとして扱い、反映は入口の再起動で行う。
    /// これを冪等にするため、登録は現在の file を lock 内で読み直してから書く。
    /// 同じ宣言でも置き場所だけは用意し直し、途中で消えた directory を残さない。
    pub fn register(
        &mut self,
        entry: ExtensionEntry,
    ) -> Result<Registration, ExtensionCatalogError> {
        fs::create_dir_all(&self.root).map_err(ExtensionCatalogError::Io)?;
        let _lock = ExclusiveFile::acquire(&self.root.join(LOCK_FILE))?;
        let mut entries = load_entries(&self.root)?;
        let result = match entries.get(&entry.id) {
            Some(current) if current == &entry => Registration::Unchanged,
            Some(_) => {
                entries.insert(entry.id.clone(), entry.clone());
                Registration::Updated
            }
            None => {
                entries.insert(entry.id.clone(), entry.clone());
                Registration::Created
            }
        };
        self.prepare_placement(&entry)?;
        if result != Registration::Unchanged {
            write_entries(&self.root, &entries)?;
        }
        self.entries = entries;
        Ok(result)
    }

    /// 導入済み package の展開先。読み書きは行わない。
    pub fn installed_directory(
        &self,
        id: &ExtensionPackageId,
    ) -> Result<PathBuf, InvalidExtensionPackageId> {
        directory_under(self.root.join(INSTALLED_DIR), id.as_str())
    }

    /// 拡張データの正本の置き場所。Client 側は再取得可能な cache にだけ持つ。
    pub fn data_directory(
        &self,
        id: &ExtensionPackageId,
    ) -> Result<PathBuf, InvalidExtensionPackageId> {
        directory_under(self.root.join(DATA_DIR), id.as_str())
    }

    /// 宣言に対応する directory を用意する。
    ///
    /// package の展開は別段階なので `installed/<id>/` はここで作らない。空の
    /// directory を置くと「package が入っている」と誤認させるため、実体を置く
    /// 段階（PR2）が作る。データの置き場所だけを先に用意する。
    ///
    /// `create_dir_all` の成功だけでは実際に作られたと判断しない。Windows の予約
    /// デバイス名など、作成したように見せて何も作らない path があり、その状態で
    /// 目録を書くと拡張データの置き場所を失う。作成後に実在を確かめる。
    fn prepare_placement(&self, entry: &ExtensionEntry) -> Result<(), ExtensionCatalogError> {
        let data = self
            .data_directory(&entry.id)
            .map_err(|_| ExtensionCatalogError::InvalidEntry)?;
        fs::create_dir_all(&data).map_err(ExtensionCatalogError::Io)?;
        if !data.is_dir() {
            return Err(ExtensionCatalogError::InvalidEntry);
        }
        Ok(())
    }
}

/// `id` を directory 名として結合する。`ExtensionPackageId` は配置に使える文字だけを
/// 許すため、ここでの再検査は将来 ID の規則が緩んだ場合の防御になる。
fn directory_under(root: PathBuf, id: &str) -> Result<PathBuf, InvalidExtensionPackageId> {
    ExtensionPackageId::try_new(id).map_err(|_| InvalidExtensionPackageId)?;
    Ok(root.join(id))
}

fn catalog_path(root: &Path) -> PathBuf {
    root.join(CATALOG_FILE)
}

fn load_entries(
    root: &Path,
) -> Result<BTreeMap<ExtensionPackageId, ExtensionEntry>, ExtensionCatalogError> {
    let bytes = match fs::read(catalog_path(root)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(ExtensionCatalogError::Io(error)),
    };
    let document: CatalogDocument =
        serde_json::from_slice(&bytes).map_err(ExtensionCatalogError::Invalid)?;
    if document.schema != CATALOG_SCHEMA {
        return Err(ExtensionCatalogError::UnsupportedSchema(document.schema));
    }
    let mut entries = BTreeMap::new();
    for entry in document.extensions {
        if entries.insert(entry.id.clone(), entry).is_some() {
            return Err(ExtensionCatalogError::InvalidEntry);
        }
    }
    Ok(entries)
}

/// 目録を原子的に置き換える。書き込み途中の値を読ませない。
fn write_entries(
    root: &Path,
    entries: &BTreeMap<ExtensionPackageId, ExtensionEntry>,
) -> Result<(), ExtensionCatalogError> {
    let document = CatalogDocument {
        schema: CATALOG_SCHEMA,
        extensions: entries.values().cloned().collect(),
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
        .persist(catalog_path(root))
        .map_err(|error| ExtensionCatalogError::Io(error.error))?;
    Ok(())
}

/// 目録の読み書きを直列化する。同じ state directory を使う 2 つの入口が
/// 互いの宣言を失わないようにするためだけの排他。
struct ExclusiveFile(fs::File);

impl ExclusiveFile {
    fn acquire(path: &Path) -> Result<Self, ExtensionCatalogError> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(ExtensionCatalogError::Io)?;
        file.lock().map_err(ExtensionCatalogError::Io)?;
        Ok(Self(file))
    }
}

impl Drop for ExclusiveFile {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Debug)]
/// 目録の読み書きに失敗した。`Invalid` は JSON、`UnsupportedSchema` は版の不一致。
pub enum ExtensionCatalogError {
    Io(io::Error),
    Invalid(serde_json::Error),
    UnsupportedSchema(u32),
    InvalidEntry,
}

impl fmt::Display for ExtensionCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "拡張の目録を読み書きできません: {error}"),
            Self::Invalid(error) => write!(formatter, "拡張の目録が不正です: {error}"),
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "未対応の拡張目録 schema です: {schema}")
            }
            Self::InvalidEntry => formatter.write_str("拡張の目録に不正な項目があります"),
        }
    }
}

impl std::error::Error for ExtensionCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(error) => Some(error),
            Self::UnsupportedSchema(_) | Self::InvalidEntry => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ExtensionPackageId {
        ExtensionPackageId::try_new(value).unwrap()
    }

    fn version(value: &str) -> ExtensionVersion {
        ExtensionVersion::parse(value).unwrap()
    }

    fn entry(name: &str, enabled: bool) -> ExtensionEntry {
        ExtensionEntry::new(
            id(name),
            version("0.1.0"),
            ExtensionState::installed(enabled),
        )
    }

    #[test]
    fn a_missing_catalog_is_empty_and_does_not_create_files() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        let catalog = ExtensionCatalog::open(directory.clone()).unwrap();
        assert_eq!(catalog.entries().count(), 0);
        assert!(!directory.exists());
    }

    #[test]
    fn registering_the_same_declaration_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
        assert_eq!(
            catalog
                .register(entry("dev.example.terminal", true))
                .unwrap(),
            Registration::Created
        );
        let first = fs::read(directory.join(CATALOG_FILE)).unwrap();

        let mut reopened = ExtensionCatalog::open(directory.clone()).unwrap();
        assert_eq!(
            reopened
                .register(entry("dev.example.terminal", true))
                .unwrap(),
            Registration::Unchanged
        );
        assert_eq!(fs::read(directory.join(CATALOG_FILE)).unwrap(), first);
        assert_eq!(reopened.entries().count(), 1);
    }

    #[test]
    fn declaration_changes_replace_the_entry_and_survive_reopening() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
        catalog
            .register(entry("dev.example.terminal", true))
            .unwrap();
        assert_eq!(
            catalog
                .register(entry("dev.example.terminal", false))
                .unwrap(),
            Registration::Updated
        );
        assert_eq!(
            catalog
                .register(ExtensionEntry::new(
                    id("dev.example.terminal"),
                    version("0.2.0"),
                    ExtensionState::installed(true),
                ))
                .unwrap(),
            Registration::Updated
        );

        let entries: Vec<_> = ExtensionCatalog::open(directory)
            .unwrap()
            .entries()
            .cloned()
            .collect();
        assert_eq!(
            entries,
            vec![ExtensionEntry::new(
                id("dev.example.terminal"),
                version("0.2.0"),
                ExtensionState::installed(true),
            )]
        );
    }

    #[test]
    fn the_saved_document_keeps_a_flat_entry_shape() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
        catalog
            .register(entry("dev.example.terminal", false))
            .unwrap();

        let saved = fs::read_to_string(directory.join(CATALOG_FILE)).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&saved).unwrap(),
            serde_json::json!({
                "schema": 1,
                "extensions": [{
                    "id": "dev.example.terminal",
                    "version": "0.1.0",
                    "installed": true,
                    "enabled": false,
                }],
            })
        );
    }

    #[test]
    fn package_and_data_live_in_separate_directories_below_the_catalog() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
        catalog
            .register(entry("dev.example.terminal", true))
            .unwrap();

        let id = id("dev.example.terminal");
        assert_eq!(
            catalog.installed_directory(&id).unwrap(),
            directory.join("installed").join("dev.example.terminal")
        );
        assert_eq!(
            catalog.data_directory(&id).unwrap(),
            directory.join("data").join("dev.example.terminal")
        );
        assert!(
            !catalog.installed_directory(&id).unwrap().exists(),
            "package の展開は読み込み段階の責務なので、目録の登録では作らない"
        );
        assert!(catalog.data_directory(&id).unwrap().is_dir());
    }

    #[test]
    fn data_placements_stay_distinct_for_ids_that_differ_only_by_a_trailing_dot() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
        catalog.register(entry("a", true)).unwrap();

        // "a." は Windows で "a" と同じ directory を指すため、目録に入れない。
        for id in ["a.", "a..", "nul", "nul.txt"] {
            assert!(
                ExtensionPackageId::try_new(id).is_err(),
                "登録に使えない ID は {id:?}"
            );
        }

        let a = id("a");
        let data = catalog.data_directory(&a).unwrap();
        assert!(data.is_dir());
        assert_eq!(
            std::fs::read_dir(directory.join("data")).unwrap().count(),
            1,
            "衝突する ID を拒否した結果、data の下は 1 件だけになる"
        );
    }

    #[test]
    fn a_declaration_without_a_package_keeps_only_the_data_directory() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
        catalog
            .register(ExtensionEntry::new(
                id("dev.example.terminal"),
                version("0.1.0"),
                ExtensionState::unavailable(),
            ))
            .unwrap();

        let id = id("dev.example.terminal");
        assert!(!catalog.installed_directory(&id).unwrap().exists());
        assert!(catalog.data_directory(&id).unwrap().is_dir());
    }

    #[test]
    fn a_placement_failure_leaves_the_catalog_untouched() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        fs::create_dir_all(&directory).unwrap();
        // `data` を file で塞ぎ、`data/<id>/` を作れない状態にする。
        fs::write(directory.join("data"), b"not a directory").unwrap();

        let mut catalog = ExtensionCatalog::open(directory.clone()).unwrap();
        assert!(catalog.register(entry("dev.example.one", true)).is_err());
        assert!(
            !directory.join(CATALOG_FILE).exists(),
            "配置を作れない登録は目録を書かない"
        );
        assert_eq!(
            fs::read(directory.join("data")).unwrap(),
            b"not a directory"
        );
        assert_eq!(catalog.entries().count(), 0);
    }

    #[test]
    fn a_broken_catalog_is_reported_and_never_replaced() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(CATALOG_FILE), b"{\"schema\":1,").unwrap();

        assert!(ExtensionCatalog::open(directory.clone()).is_err());
        assert!(matches!(
            ExtensionCatalog::open(directory.clone()),
            Err(ExtensionCatalogError::Invalid(_))
        ));
        assert_eq!(
            fs::read(directory.join(CATALOG_FILE)).unwrap(),
            b"{\"schema\":1,"
        );
    }

    #[test]
    fn a_later_schema_is_not_read_as_the_current_one() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(CATALOG_FILE),
            br#"{"schema":2,"extensions":[]}"#,
        )
        .unwrap();

        assert!(matches!(
            ExtensionCatalog::open(directory),
            Err(ExtensionCatalogError::UnsupportedSchema(2))
        ));
    }

    #[test]
    fn inconsistent_or_duplicate_entries_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        fs::create_dir_all(&directory).unwrap();
        for payload in [
            r#"{"schema":1,"extensions":[{"id":"a","version":"1","installed":false,"enabled":true}]}"#,
            r#"{"schema":1,"extensions":[
                {"id":"a","version":"1","installed":true,"enabled":true},
                {"id":"a","version":"2","installed":true,"enabled":false}
            ]}"#,
            r#"{"schema":1,"extensions":[{"id":"../escape","version":"1","installed":true,"enabled":true}]}"#,
            r#"{"schema":1,"extensions":[{"id":"a","version":"1","installed":true,"enabled":true,"extra":1}]}"#,
        ] {
            fs::write(directory.join(CATALOG_FILE), payload).unwrap();
            assert!(
                ExtensionCatalog::open(directory.clone()).is_err(),
                "must reject {payload}"
            );
        }
    }

    #[test]
    fn concurrent_registrations_keep_both_declarations() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("extensions");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let run = |name: &'static str| {
            let directory = directory.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut catalog = ExtensionCatalog::open(directory).unwrap();
                barrier.wait();
                catalog.register(entry(name, true)).unwrap();
            })
        };
        let first = run("dev.example.one");
        let second = run("dev.example.two");
        first.join().unwrap();
        second.join().unwrap();

        let entries: Vec<_> = ExtensionCatalog::open(directory)
            .unwrap()
            .entries()
            .map(|entry| entry.id.as_str().to_owned())
            .collect();
        assert_eq!(entries, vec!["dev.example.one", "dev.example.two"]);
    }
}
