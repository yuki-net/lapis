//! `.lapis` package（zip + manifest）の読み込み。
//!
//! package の種別は manifest の `kind` が決める。拡張子や招待 file（`.lapis-host`）の
//! 名前では区別しない。読み込みは次の順で行い、途中で失敗したら配置を変えない。
//!
//! 1. zip として読み、member 名・重複・上限を検証する
//! 2. `manifest.json` の形（未知・欠落 field、型、値の範囲）を検証する
//! 3. `key_id` が信頼済み registry にあり、Ed25519 署名が正しいことを検証する
//! 4. manifest が宣言する全 file の SHA-256 を zip の内容と照合する
//! 5. manifest が要求する契約と版を Host の提供値と照合する
//! 6. 導入済みの置き場所へ展開し、目録へ宣言を登録する
//!
//! 署名が不正な package は目録にも載せない（manifest の `id` 自体を信頼できないため）。
//! 互換性がない package は展開せず、目録に「非互換」として登録する。

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    ExtensionCatalog, ExtensionCatalogError, ExtensionEntry, ExtensionPackageId, ExtensionState,
    ExtensionVersion, Registration,
    compatibility::{ContractRequirement, HostContracts, Incompatibility},
    journal::{PendingOperation, PendingPhase},
    signature::{SignatureError, TrustedKeys},
};

/// manifest の schema。項目を増やすときは schema を上げ、旧 schema は受理しない。
pub const PACKAGE_SCHEMA: i64 = 1;

/// capability 宣言を含む package schema。schema 1 も読み取る。
pub const CAPABILITY_PACKAGE_SCHEMA: i64 = 2;

/// package 内で manifest を置く名前。manifest が宣言する file には使えない。
pub const MANIFEST_NAME: &str = "manifest.json";

/// package 1 つと、その展開後の合計の上限。読み込み時に確保する memory を決める。
const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FILES: usize = 1024;
const MAX_PATH_BYTES: usize = 255;

const MANIFEST_FIELDS: [&str; 8] = [
    "schema",
    "kind",
    "id",
    "version",
    "requires",
    "files",
    "key_id",
    "signature",
];
const REQUIREMENT_FIELDS: [&str; 3] = ["contract", "minimum", "maximum"];
const FILE_FIELDS: [&str; 2] = ["path", "sha256"];

/// package の種別。`.lapis` は同じ zip 形式で、manifest のこの値だけが種別を決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    /// 拡張 package。この crate が読み込む対象。
    Extension,
    /// 拡張ではない package。読み込みは別段階の責務であり、拡張としては扱わない。
    Host,
}

impl PackageKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "extension" => Some(Self::Extension),
            "host" => Some(Self::Host),
            _ => None,
        }
    }
}

/// manifest が宣言する 1 file。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageFile {
    pub path: String,
    pub sha256: String,
}

/// 検証済み manifest。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageManifest {
    pub kind: PackageKind,
    pub id: ExtensionPackageId,
    pub version: ExtensionVersion,
    pub requires: Vec<ContractRequirement>,
    /// 拡張が利用を希望する能力。宣言だけでは権限を与えない。
    pub capabilities: Vec<PackageCapability>,
    pub files: Vec<PackageFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageCapability {
    pub id: crate::ContractId,
    pub version: semver::Version,
}

/// 署名と内容を検証した導入済み package のスナップショット。
/// 実行側は検証後に path を読み直さず、この bytes を使用する。
#[derive(Debug)]
pub struct VerifiedPackage {
    pub manifest: PackageManifest,
    pub files: BTreeMap<String, Vec<u8>>,
    manifest_digest: String,
    verified_id: ExtensionPackageId,
    verified_capabilities: Vec<PackageCapability>,
    signature_verified: bool,
}

impl VerifiedPackage {
    /// 署名省略の開発用 package と、信頼済み鍵で署名検証した package を区別する。
    pub fn signature_verified(&self) -> bool {
        self.signature_verified
    }
    /// 署名とファイルを検証した manifest の digest。承認を正確な package に結び付ける。
    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub(crate) fn verified_id(&self) -> &ExtensionPackageId {
        &self.verified_id
    }
    pub(crate) fn declares(&self, capability: &PackageCapability) -> bool {
        self.verified_capabilities.contains(capability)
    }
}

/// 読み込みの結果。目録に載った宣言の内容を返す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    /// 展開し、目録へ導入済みの宣言を登録した。
    Installed {
        id: ExtensionPackageId,
        version: ExtensionVersion,
        enabled: bool,
    },
    /// 互換性がなく、展開しなかった。
    ///
    /// 目録に導入済みの実体が無ければ「非互換」として宣言し、あればその宣言を残す。
    /// 動いている版を、入れようとした非互換の版のために壊さないため。
    Incompatible {
        id: ExtensionPackageId,
        version: ExtensionVersion,
        reason: Incompatibility,
        /// 導入済みの宣言と実体を残したか。true なら目録は変えていない。
        kept_previous: bool,
    },
}

impl InstallOutcome {
    pub fn id(&self) -> &ExtensionPackageId {
        match self {
            Self::Installed { id, .. } | Self::Incompatible { id, .. } => id,
        }
    }

    pub fn is_installed(&self) -> bool {
        matches!(self, Self::Installed { .. })
    }
}

/// package の読み込みに失敗した。目録は変更していない。
#[derive(Debug)]
pub enum PackageError {
    /// package を読めない。
    Io(io::Error),
    /// zip・manifest の形が不正。
    Malformed(String),
    /// 署名・信頼済み registry が不正。宣言の `id` も信頼できないため目録に載せない。
    Signature(SignatureError),
    /// 拡張以外の package。読まずに拒否する。
    NotAnExtension(PackageKind),
    /// 目録の読み書きに失敗した。
    Catalog(ExtensionCatalogError),
    /// 中断した導入・除去の回復に失敗した。目録と実体が食い違ったままである。
    ///
    /// 呼び出し側は目録を宣言として公開してはいけない。journal は残っているので、
    /// 次の起動で回復を再試行できる。
    Unrecovered { cause: String, recovery: String },
}

impl PackageError {
    /// 目録と実体が食い違ったまま残っているか。
    ///
    /// true の間は、目録を宣言として Client へ配ってはいけない。
    pub fn leaves_inconsistent_state(&self) -> bool {
        matches!(self, Self::Unrecovered { .. })
    }
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "拡張 package を読めません: {error}"),
            Self::Malformed(message) => write!(formatter, "拡張 package が不正です: {message}"),
            Self::Signature(error) => {
                write!(formatter, "拡張 package の署名を検証できません: {error}")
            }
            Self::NotAnExtension(kind) => {
                write!(formatter, "拡張 package ではありません: {kind:?}")
            }
            Self::Catalog(error) => write!(formatter, "{error}"),
            Self::Unrecovered { cause, recovery } => write!(
                formatter,
                "拡張の状態を回復できません（{recovery}）。元の失敗: {cause}"
            ),
        }
    }
}

impl std::error::Error for PackageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Signature(error) => Some(error),
            Self::Catalog(error) => Some(error),
            Self::Malformed(_) | Self::NotAnExtension(_) | Self::Unrecovered { .. } => None,
        }
    }
}

impl From<ExtensionCatalogError> for PackageError {
    fn from(error: ExtensionCatalogError) -> Self {
        Self::Catalog(error)
    }
}

impl From<crate::InvalidExtensionPackageId> for PackageError {
    fn from(error: crate::InvalidExtensionPackageId) -> Self {
        Self::Malformed(error.to_string())
    }
}

/// package の読み込みと、読み込んだ拡張の状態変更。
///
/// 信頼済み公開鍵と、Host が提供する契約の版を所有する。Host は
/// [`ExtensionLoader::repository`] を使い、信頼の起点を repository の
/// `release/trusted-keys.json` だけに保つ。
pub struct ExtensionLoader {
    keys: TrustedKeys,
    contracts: HostContracts,
    allow_unsigned_development: bool,
}

impl ExtensionLoader {
    /// repository の registry と、この Host が提供する契約を使う。
    pub fn repository() -> Result<Self, SignatureError> {
        Ok(Self {
            keys: TrustedKeys::repository()?,
            contracts: HostContracts::current(),
            allow_unsigned_development: false,
        })
    }

    /// Host 管理者が明示した発行者鍵を、組み込みの信頼済み鍵に追加する。
    pub fn repository_with_additional_keys(raw: &[u8]) -> Result<Self, SignatureError> {
        let mut keys = TrustedKeys::repository()?;
        keys.add_registry(raw)?;
        Ok(Self {
            keys,
            contracts: HostContracts::current(),
            allow_unsigned_development: false,
        })
    }

    /// 信頼済み公開鍵と提供する契約を明示して作る。検証規則を確かめるテストが使う。
    pub fn new(keys: TrustedKeys, contracts: HostContracts) -> Self {
        Self {
            keys,
            contracts,
            allow_unsigned_development: false,
        }
    }

    /// 明示的なローカル開発時のみ署名のない package を許可する。
    /// 署名がある場合は開発時も必ず検証する。
    pub fn allow_unsigned_local_development(mut self) -> Self {
        self.allow_unsigned_development = true;
        self
    }

    /// 導入済み package の署名・目録・契約・各 file を再検証する。
    /// 旧版で manifest を保存していない package は再導入が必要。
    pub fn load_installed(
        &self,
        catalog: &mut ExtensionCatalog,
        id: &ExtensionPackageId,
    ) -> Result<VerifiedPackage, PackageError> {
        self.read_installed(catalog, id, true)
    }

    /// 署名と内容を検証して能力を表示する。実行互換性は付与しない。
    pub fn inspect_installed(
        &self,
        catalog: &mut ExtensionCatalog,
        id: &ExtensionPackageId,
    ) -> Result<VerifiedPackage, PackageError> {
        self.read_installed(catalog, id, false)
    }

    /// 表示用に読み取った package が現在の Host で実行可能かを照合する。
    pub fn is_compatible(&self, package: &VerifiedPackage) -> bool {
        self.contracts.check(&package.manifest.requires).is_ok()
    }

    fn read_installed(
        &self,
        catalog: &mut ExtensionCatalog,
        id: &ExtensionPackageId,
        require_compatible: bool,
    ) -> Result<VerifiedPackage, PackageError> {
        let _lock = lock_install(catalog.root())?;
        crate::journal::recover_locked(catalog)?;
        catalog.reload()?;
        let entry = catalog
            .get(id)
            .ok_or_else(|| PackageError::Malformed(format!("拡張が目録にありません: {id}")))?;
        if !entry.state.is_installed() {
            return Err(PackageError::Malformed(format!(
                "拡張が導入されていません: {id}"
            )));
        }
        let directory = catalog.installed_directory(id)?;
        let raw = read_installed_file(&directory, MANIFEST_NAME)?;
        let manifest = parse_manifest(&raw, &self.keys, self.allow_unsigned_development)?;
        if manifest.kind != PackageKind::Extension {
            return Err(PackageError::NotAnExtension(manifest.kind));
        }
        if &manifest.id != id || manifest.version != entry.version {
            return Err(PackageError::Malformed(
                "目録と manifest が一致しません".to_owned(),
            ));
        }
        if manifest.files.len() >= MAX_FILES {
            return Err(PackageError::Malformed(
                "package の file 数が上限を超えています".to_owned(),
            ));
        }
        if require_compatible {
            self.contracts
                .check(&manifest.requires)
                .map_err(|error| PackageError::Malformed(format!("拡張が非互換です: {error}")))?;
        }
        let mut total = raw.len() as u64;
        let mut files = BTreeMap::new();
        for file in &manifest.files {
            let bytes = read_installed_file(&directory, &file.path)?;
            total += bytes.len() as u64;
            if total > MAX_PACKAGE_BYTES {
                return Err(PackageError::Malformed(
                    "package の展開後の大きさが上限を超えています".to_owned(),
                ));
            }
            files.insert(file.path.clone(), bytes);
        }
        verify_files(&manifest, &files)?;
        // parse_manifest は key_id が存在すれば必ず署名を検証する。
        let signature_verified = serde_json::from_slice::<Value>(&raw)
            .map_err(|error| PackageError::Malformed(error.to_string()))?
            .get("key_id")
            .is_some();
        Ok(VerifiedPackage {
            verified_id: manifest.id.clone(),
            verified_capabilities: manifest.capabilities.clone(),
            manifest,
            files,
            manifest_digest: format!("{:x}", Sha256::digest(&raw)),
            signature_verified,
        })
    }

    /// package を検証して導入し、目録へ宣言を登録する。
    ///
    /// `enabled` は登録する宣言の有効状態であり、反映は入口の再起動で行う。同じ package の
    /// 再導入は同じ宣言の再登録になるため、目録の file を書き換えない。
    pub fn install(
        &self,
        catalog: &mut ExtensionCatalog,
        package: &Path,
        enabled: bool,
    ) -> Result<InstallOutcome, PackageError> {
        self.install_inner(catalog, package, enabled, false)
            .map(|outcome| outcome.expect("通常の導入は必ず結果を返す"))
    }

    /// 同梱の初回導入。削除済みを含む既存の目録を上書きしない。
    pub fn install_if_missing(
        &self,
        catalog: &mut ExtensionCatalog,
        package: &Path,
        enabled: bool,
    ) -> Result<Option<InstallOutcome>, PackageError> {
        self.install_inner(catalog, package, enabled, true)
    }

    fn install_inner(
        &self,
        catalog: &mut ExtensionCatalog,
        package: &Path,
        enabled: bool,
        only_missing: bool,
    ) -> Result<Option<InstallOutcome>, PackageError> {
        let archive = read_archive(package)?;
        let manifest_value = archive
            .get(MANIFEST_NAME)
            .ok_or_else(|| PackageError::Malformed(format!("{MANIFEST_NAME} がない")))?;
        let manifest = parse_manifest(manifest_value, &self.keys, self.allow_unsigned_development)?;
        if manifest.kind != PackageKind::Extension {
            return Err(PackageError::NotAnExtension(manifest.kind));
        }
        verify_files(&manifest, &archive)?;

        let _lock = lock_install(catalog.root())?;
        // 前回の中断を先に回復する。目録と実体が食い違ったまま判断しない。
        // lock は取得済みなので、入れ子にせず `recover_locked` を呼ぶ。
        crate::journal::recover_locked(catalog)?;
        // 別の入口が同じ目録を変えている可能性がある。判断の前に file を正本にする。
        catalog.reload()?;
        if only_missing && catalog.get(&manifest.id).is_some() {
            return Ok(None);
        }
        match self.contracts.check(&manifest.requires) {
            Ok(()) => {
                // 展開先の実体を先に用意し、目録を書けたときだけ置き換える。目録を
                // 書けない場合に新しい実体だけを残すと「package が入っている」と誤認
                // させるため、置き換えは目録の書き込み後に行う。
                let previous = catalog.get(&manifest.id).cloned();
                let staged = stage(catalog, &manifest, &archive)?;
                // ここから先で終了しても、次の起動が journal を見て段階に応じて回復する。
                // 記録は「前の宣言」であり、取り消すときに戻す値になる。
                if let Err(error) = crate::journal::begin(
                    catalog.root(),
                    &manifest.id,
                    PendingOperation::Install,
                    previous.clone(),
                ) {
                    let _ = fs::remove_dir_all(&staged);
                    return Err(PackageError::Catalog(error));
                }
                let result = (|| -> Result<(), PackageError> {
                    catalog.register(ExtensionEntry::new(
                        manifest.id.clone(),
                        manifest.version.clone(),
                        ExtensionState::installed(enabled),
                    ))?;
                    crate::journal::advance(
                        catalog.root(),
                        &manifest.id,
                        PendingPhase::CatalogWritten,
                    )?;
                    // 退避まで進めたら段階を記録し、その後で新しい実体を置く。
                    // 退避したまま落ちても、回復が旧版を戻せるようにするため。
                    backup(catalog, &manifest.id)?;
                    crate::journal::advance(catalog.root(), &manifest.id, PendingPhase::BackedUp)?;
                    place(catalog, &manifest.id, &staged)?;
                    crate::journal::advance(catalog.root(), &manifest.id, PendingPhase::Swapped)?;
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        // 確定後に退避した旧版を片付ける。失敗しても実体は新しく、
                        // 次の回復が `.swap` を掃除する。
                        if let Err(error) =
                            remove_directory(&swap_path(catalog.root(), &manifest.id))
                        {
                            eprintln!("拡張 {} の退避した旧版を消せません: {error}", manifest.id);
                        }
                        crate::journal::finish(catalog.root(), &manifest.id)?;
                        Ok(Some(InstallOutcome::Installed {
                            id: manifest.id,
                            version: manifest.version,
                            enabled,
                        }))
                    }
                    Err(error) => {
                        // 実体を置き換えられなかった。今の段階から回復して、目録と実体を
                        // 操作前へ戻す。回復できなければ、目録を宣言として配ってはいけない
                        // 状態であることを呼び出し側へ伝える。
                        let _ = fs::remove_dir_all(&staged);
                        match crate::journal::recover_locked(catalog) {
                            Ok(_) => Err(error),
                            Err(recovery) => Err(PackageError::Unrecovered {
                                cause: error.to_string(),
                                recovery: recovery.to_string(),
                            }),
                        }
                    }
                }
            }
            Err(reason) => {
                // 導入済みの版が動いているなら、その宣言と実体を残す。非互換の版を
                // 入れようとしたために、動いている版を失わせない。
                let previous = catalog
                    .get(&manifest.id)
                    .filter(|entry| entry.state.is_installed())
                    .cloned();
                let kept_previous = previous.is_some();
                if previous.is_none() {
                    // 目録を先に「非互換」へ変え、その後に旧版の実体を消す。順序を逆に
                    // すると、目録を書けなかったときに「導入済みなのに実体が無い」状態を
                    // 残す。拡張データは再導入で引き継げるよう消さない。
                    catalog.register(ExtensionEntry::new(
                        manifest.id.clone(),
                        manifest.version.clone(),
                        ExtensionState::incompatible(),
                    ))?;
                    remove_directory(&catalog.installed_directory(&manifest.id)?)?;
                }
                Ok(Some(InstallOutcome::Incompatible {
                    id: manifest.id,
                    version: manifest.version,
                    reason,
                    kept_previous,
                }))
            }
        }
    }

    /// 有効化・無効化を目録の宣言として登録する。
    ///
    /// 反映は入口の再起動で行う。未導入・非互換の拡張と、目録に無い ID は `None`。
    pub fn set_enabled(
        &self,
        catalog: &mut ExtensionCatalog,
        id: &ExtensionPackageId,
        enabled: bool,
    ) -> Result<Option<Registration>, PackageError> {
        let _lock = lock_install(catalog.root())?;
        Ok(catalog.set_enabled(id, enabled)?)
    }

    /// 導入済み package を取り除き、宣言を「未導入」にする。
    ///
    /// 拡張データ（`data/<id>/`）は消さない。再導入した拡張が同じデータを引き継ぐ。
    ///
    /// 実体は先に退避し、目録の更新が成功してから消す。目録を書けなかったときに
    /// 実体だけを失わないため。
    pub fn remove(
        &self,
        catalog: &mut ExtensionCatalog,
        id: &ExtensionPackageId,
    ) -> Result<(), PackageError> {
        let _lock = lock_install(catalog.root())?;
        // 前回の中断を先に回復する。目録と実体が食い違ったまま判断しない。
        // lock は取得済みなので、入れ子にせず `recover_locked` を呼ぶ。
        crate::journal::recover_locked(catalog)?;
        // 別の入口が同じ目録を変えている可能性がある。判断の前に file を正本にする。
        catalog.reload()?;
        let entry = catalog
            .get(id)
            .cloned()
            .ok_or_else(|| PackageError::Malformed(format!("目録に {id} がない")))?;
        // 除去が途中で失敗しても承認は取り消したままにする（安全側に倒す）。
        crate::CapabilityApprovals::new(catalog.root().to_owned())
            .revoke_extension(id)
            .map_err(PackageError::Io)?;
        // 退避より先に記録する。退避したまま落ちても回復が実体を戻せるようにするため。
        crate::journal::begin(
            catalog.root(),
            id,
            PendingOperation::Remove,
            Some(entry.clone()),
        )?;
        let result = (|| -> Result<(), PackageError> {
            backup_removal(catalog, id)?;
            crate::journal::advance(catalog.root(), id, PendingPhase::BackedUp)?;
            catalog.register(ExtensionEntry::new(
                entry.id.clone(),
                entry.version,
                ExtensionState::unavailable(),
            ))?;
            crate::journal::advance(catalog.root(), id, PendingPhase::Swapped)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                remove_directory(&swap_path(catalog.root(), id))?;
                crate::journal::finish(catalog.root(), id)?;
                Ok(())
            }
            Err(error) => {
                // 導入と同じ回復経路に委ねる。段階に応じて実体と宣言を操作前へ戻す。
                // 回復できなければ、目録を配ってはいけない状態であることを伝える。
                match crate::journal::recover_locked(catalog) {
                    Ok(_) => Err(error),
                    Err(recovery) => Err(PackageError::Unrecovered {
                        cause: error.to_string(),
                        recovery: recovery.to_string(),
                    }),
                }
            }
        }
    }
}

/// 除去のために実体を `.swap/<id>` へ退避する。実体が無ければ何もしない。
fn backup_removal(catalog: &ExtensionCatalog, id: &ExtensionPackageId) -> Result<(), PackageError> {
    let target = catalog.installed_directory(id)?;
    if !target.is_dir() {
        return Ok(());
    }
    let backup = swap_path(catalog.root(), id);
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent).map_err(PackageError::Io)?;
    }
    remove_directory(&backup)?;
    fs::rename(&target, &backup).map_err(PackageError::Io)
}

fn remove_directory(path: &Path) -> Result<(), PackageError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PackageError::Io(error)),
    }
}

fn read_installed_file(directory: &Path, name: &str) -> Result<Vec<u8>, PackageError> {
    if name != MANIFEST_NAME {
        validate_member_path(name)?;
    }
    let mut path = directory.to_path_buf();
    if fs::symlink_metadata(&path)
        .map_err(PackageError::Io)?
        .file_type()
        .is_symlink()
    {
        return Err(PackageError::Malformed("導入先が symlink です".to_owned()));
    }
    for part in name.split('/') {
        path.push(part);
        if fs::symlink_metadata(&path)
            .map_err(PackageError::Io)?
            .file_type()
            .is_symlink()
        {
            return Err(PackageError::Malformed(format!(
                "symlink は読み込めません: {name}"
            )));
        }
    }
    let file = fs::File::open(&path).map_err(PackageError::Io)?;
    if !file.metadata().map_err(PackageError::Io)?.is_file() {
        return Err(PackageError::Malformed(format!(
            "file ではありません: {name}"
        )));
    }
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut io::Read::take(file, MAX_FILE_BYTES + 1), &mut bytes)
        .map_err(PackageError::Io)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(PackageError::Malformed(format!(
            "file が大きすぎます: {name}"
        )));
    }
    Ok(bytes)
}

/// package の file 名 → 内容。zip member の検証をここで済ませる。
fn read_archive(package: &Path) -> Result<BTreeMap<String, Vec<u8>>, PackageError> {
    let metadata = fs::metadata(package).map_err(PackageError::Io)?;
    if !metadata.is_file() {
        return Err(PackageError::Malformed("package が file でない".to_owned()));
    }
    if metadata.len() > MAX_PACKAGE_BYTES {
        return Err(PackageError::Malformed(format!(
            "package が大きすぎます（上限 {MAX_PACKAGE_BYTES} byte）"
        )));
    }
    let file = fs::File::open(package).map_err(PackageError::Io)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| PackageError::Malformed(format!("zip として読めません: {error}")))?;
    // `zip` 9.5.1 は同名 member を内部 map に集約するため、`len()` と `by_index()` からは
    // 2 つ目以降が見えない。後段の重複検査だけでは足りないので、走査前に拒否する。
    if zip.has_duplicate_names() {
        return Err(PackageError::Malformed(
            "package に同名の member がある".to_owned(),
        ));
    }
    if zip.len() > MAX_FILES {
        return Err(PackageError::Malformed(format!(
            "package の file 数が多すぎます（上限 {MAX_FILES}）"
        )));
    }
    let mut entries = BTreeMap::new();
    // case を区別しないファイルシステムでは `A.txt` と `a.txt` が同じ file になる。
    let mut folded = BTreeMap::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| PackageError::Malformed(format!("member を読めません: {error}")))?;
        let name = member_name(&entry)?;
        if entry.is_dir() {
            continue;
        }
        if entry.is_symlink() {
            return Err(PackageError::Malformed(format!("symlink がある: {name}")));
        }
        if entry.size() > MAX_FILE_BYTES {
            return Err(PackageError::Malformed(format!(
                "{name} が大きすぎます（上限 {MAX_FILE_BYTES} byte）"
            )));
        }
        if let Some(previous) = folded.insert(name.to_lowercase(), name.clone()) {
            return Err(PackageError::Malformed(format!(
                "同じ file を指す path がある: {previous} と {name}"
            )));
        }
        // `entry.size()` は central directory の値であり、展開後の実際の長さを保証しない。
        // 上限を超えて読まないよう `take` で切り、超えたら失敗させる。
        let mut bytes = Vec::with_capacity(entry.size().min(MAX_FILE_BYTES) as usize);
        let read = io::Read::read_to_end(
            &mut io::Read::take(&mut entry, MAX_FILE_BYTES + 1),
            &mut bytes,
        )
        .map_err(|error| PackageError::Malformed(format!("{name} を読めません: {error}")))?;
        if read as u64 > MAX_FILE_BYTES {
            return Err(PackageError::Malformed(format!(
                "{name} が大きすぎます（上限 {MAX_FILE_BYTES} byte）"
            )));
        }
        total += bytes.len() as u64;
        if total > MAX_PACKAGE_BYTES {
            return Err(PackageError::Malformed(
                "package の展開後の大きさが上限を超えています".to_owned(),
            ));
        }
        if entries.insert(name.clone(), bytes).is_some() {
            return Err(PackageError::Malformed(format!(
                "重複した path がある: {name}"
            )));
        }
    }
    Ok(entries)
}

/// entry 名を検証して返す。directory entry の末尾 `/` は除く。
///
/// 規則は launcher（`clients/desktop/launcher/src/archive.rs`）が package に課すものと
/// 同じで、絶対 path・`..`・drive 指定・`\` を拒否する。
fn member_name(entry: &zip::read::ZipFile<'_, fs::File>) -> Result<String, PackageError> {
    let raw = std::str::from_utf8(entry.name_raw())
        .map_err(|_| PackageError::Malformed("member 名が UTF-8 でない".to_owned()))?;
    let name = raw.trim_end_matches('/');
    if name.is_empty()
        || name.len() > MAX_PATH_BYTES
        || name.starts_with('/')
        || raw.contains('\\')
        || name.contains(':')
        || name.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with('.')
                || part.ends_with(' ')
        })
    {
        return Err(PackageError::Malformed(format!(
            "不正な path がある: {raw}"
        )));
    }
    Ok(name.to_owned())
}

fn parse_manifest(
    raw: &[u8],
    keys: &TrustedKeys,
    allow_unsigned: bool,
) -> Result<PackageManifest, PackageError> {
    let value: Value = serde_json::from_slice(raw)
        .map_err(|error| PackageError::Malformed(format!("manifest が JSON でない: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| PackageError::Malformed("manifest が object でない".to_owned()))?;
    let unsigned = !object.contains_key("key_id") && !object.contains_key("signature");
    let schema = object.get("schema").and_then(Value::as_i64);
    if !matches!(schema, Some(PACKAGE_SCHEMA | CAPABILITY_PACKAGE_SCHEMA)) {
        return Err(PackageError::Malformed(format!(
            "未対応の schema: {}",
            object.get("schema").unwrap_or(&Value::Null)
        )));
    }
    let mut fields = if unsigned && allow_unsigned {
        MANIFEST_FIELDS[..6].to_vec()
    } else {
        MANIFEST_FIELDS.to_vec()
    };
    if schema == Some(CAPABILITY_PACKAGE_SCHEMA) {
        fields.push("capabilities");
    }
    reject_unknown_and_missing(object, &fields, "manifest")?;
    if !unsigned || !allow_unsigned {
        let key_id = object["key_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| PackageError::Malformed("key_id が空".to_owned()))?;
        let signature = object["signature"]
            .as_str()
            .ok_or_else(|| PackageError::Malformed("signature が文字列でない".to_owned()))?;
        let mut payload = object.clone();
        payload.remove("signature");
        crate::signature::verify_signature(keys, key_id, signature, &Value::Object(payload))
            .map_err(PackageError::Signature)?;
    }

    let kind = object["kind"]
        .as_str()
        .and_then(PackageKind::parse)
        .ok_or_else(|| PackageError::Malformed("kind が extension / host でない".to_owned()))?;
    let id: ExtensionPackageId = serde_json::from_value(object["id"].clone())
        .map_err(|error| PackageError::Malformed(format!("id が不正: {error}")))?;
    let version: ExtensionVersion = serde_json::from_value(object["version"].clone())
        .map_err(|error| PackageError::Malformed(format!("version が不正: {error}")))?;
    let requires = parse_requirements(&object["requires"])?;
    let capabilities = if schema == Some(CAPABILITY_PACKAGE_SCHEMA) {
        parse_capabilities(&object["capabilities"])?
    } else {
        Vec::new()
    };
    let files = parse_files(&object["files"])?;
    Ok(PackageManifest {
        kind,
        id,
        version,
        requires,
        capabilities,
        files,
    })
}

fn parse_capabilities(value: &Value) -> Result<Vec<PackageCapability>, PackageError> {
    let entries = value
        .as_array()
        .filter(|entries| entries.len() <= 64)
        .ok_or_else(|| {
            PackageError::Malformed("capabilities は 64 件以下の配列が必要です".into())
        })?;
    let mut capabilities = Vec::with_capacity(entries.len());
    for entry in entries {
        let object = entry
            .as_object()
            .ok_or_else(|| PackageError::Malformed("capability は object が必要です".into()))?;
        reject_unknown_and_missing(object, &["id", "version"], "capability")?;
        let id: crate::ContractId =
            serde_json::from_value(object["id"].clone()).map_err(|error| {
                PackageError::Malformed(format!("capability ID が不正です: {error}"))
            })?;
        let raw_version = object["version"]
            .as_str()
            .ok_or_else(|| PackageError::Malformed("capability version が不正です".into()))?;
        let version = semver::Version::parse(raw_version).map_err(|error| {
            PackageError::Malformed(format!("capability version が不正です: {error}"))
        })?;
        if version.to_string() != raw_version
            || !version.pre.is_empty()
            || !version.build.is_empty()
            || version.major == 0
        {
            return Err(PackageError::Malformed(
                "capability version は正式な SemVer が必要です".into(),
            ));
        }
        let capability = PackageCapability { id, version };
        if capabilities.contains(&capability) {
            return Err(PackageError::Malformed(
                "capability が重複しています".into(),
            ));
        }
        capabilities.push(capability);
    }
    Ok(capabilities)
}

fn parse_requirements(value: &Value) -> Result<Vec<ContractRequirement>, PackageError> {
    let array = value
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| PackageError::Malformed("requires が空".to_owned()))?;
    let mut requirements = Vec::with_capacity(array.len());
    for item in array {
        let object = item
            .as_object()
            .ok_or_else(|| PackageError::Malformed("requires の項目が object でない".to_owned()))?;
        reject_unknown_and_missing(object, &REQUIREMENT_FIELDS, "requires")?;
        let contract: crate::ContractId = serde_json::from_value(object["contract"].clone())
            .map_err(|error| PackageError::Malformed(format!("contract が不正: {error}")))?;
        let minimum = semver::Version::parse(
            object["minimum"]
                .as_str()
                .ok_or_else(|| PackageError::Malformed("minimum が文字列でない".to_owned()))?,
        )
        .map_err(|error| PackageError::Malformed(format!("minimum が version でない: {error}")))?;
        let maximum = semver::Version::parse(
            object["maximum"]
                .as_str()
                .ok_or_else(|| PackageError::Malformed("maximum が文字列でない".to_owned()))?,
        )
        .map_err(|error| PackageError::Malformed(format!("maximum が version でない: {error}")))?;
        let requirement = ContractRequirement::new(contract, minimum, maximum)
            .map_err(|_| PackageError::Malformed("requires の版の範囲が逆転している".to_owned()))?;
        requirements.push(requirement);
    }
    Ok(requirements)
}

fn parse_files(value: &Value) -> Result<Vec<PackageFile>, PackageError> {
    let array = value
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| PackageError::Malformed("files が空".to_owned()))?;
    let mut files = Vec::with_capacity(array.len());
    let mut seen = BTreeMap::new();
    for item in array {
        let object = item
            .as_object()
            .ok_or_else(|| PackageError::Malformed("files の項目が object でない".to_owned()))?;
        reject_unknown_and_missing(object, &FILE_FIELDS, "files")?;
        let path = object["path"]
            .as_str()
            .ok_or_else(|| PackageError::Malformed("path が文字列でない".to_owned()))?;
        validate_member_path(path)?;
        let sha256 = object["sha256"]
            .as_str()
            .filter(|digest| valid_sha256(digest))
            .ok_or_else(|| PackageError::Malformed(format!("sha256 が不正: {path}")))?;
        if seen.insert(path.to_owned(), ()).is_some() {
            return Err(PackageError::Malformed(format!(
                "同じ path を 2 回宣言している: {path}"
            )));
        }
        files.push(PackageFile {
            path: path.to_owned(),
            sha256: sha256.to_owned(),
        });
    }
    Ok(files)
}

fn verify_files(
    manifest: &PackageManifest,
    archive: &BTreeMap<String, Vec<u8>>,
) -> Result<(), PackageError> {
    let declared: BTreeMap<&str, ()> = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), ()))
        .collect();
    for file in &manifest.files {
        let bytes = archive.get(&file.path).ok_or_else(|| {
            PackageError::Malformed(format!("宣言された file がない: {}", file.path))
        })?;
        if sha256_hex(bytes) != file.sha256 {
            return Err(PackageError::Malformed(format!(
                "SHA-256 が一致しない: {}",
                file.path
            )));
        }
    }
    // 宣言していない file を展開すると、manifest の署名が及ばない内容を置くことになる。
    for name in archive.keys() {
        if name == MANIFEST_NAME || declared.contains_key(name.as_str()) {
            continue;
        }
        return Err(PackageError::Malformed(format!(
            "宣言されていない file がある: {name}"
        )));
    }
    Ok(())
}

/// 検証済みの内容を一時 directory に書く。置き換えは [`swap`] で行う。
///
/// 一時 directory を導入済みの置き場所と同じ親に作るため、`rename` が同じ volume 内で
/// 完結する。
fn stage(
    catalog: &ExtensionCatalog,
    manifest: &PackageManifest,
    archive: &BTreeMap<String, Vec<u8>>,
) -> Result<PathBuf, PackageError> {
    let target = catalog.installed_directory(&manifest.id)?;
    let parent = target
        .parent()
        .ok_or_else(|| PackageError::Malformed("展開先を解決できない".to_owned()))?;
    fs::create_dir_all(parent).map_err(PackageError::Io)?;
    let staging = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(parent)
        .map_err(PackageError::Io)?;
    // 起動時に署名と契約を再検証できるよう、検証済み manifest も実体と保存する。
    let manifest_bytes = archive
        .get(MANIFEST_NAME)
        .ok_or_else(|| PackageError::Malformed(format!("{MANIFEST_NAME} がない")))?;
    fs::write(staging.path().join(MANIFEST_NAME), manifest_bytes).map_err(PackageError::Io)?;
    for file in &manifest.files {
        let bytes = archive
            .get(&file.path)
            .ok_or_else(|| PackageError::Malformed(format!("{} がない", file.path)))?;
        let path = staging.path().join(&file.path);
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory).map_err(PackageError::Io)?;
        }
        fs::write(&path, bytes).map_err(PackageError::Io)?;
    }
    Ok(staging.keep())
}

/// 旧版の実体を `.swap/<id>` へ退避する。新しい実体はまだ置かない。
///
/// 退避先は `<root>/.swap/<id>` とする。`installed/<id>` の隣に固定名で置くと、その名前が
/// 別の拡張 ID と衝突し得るため、ID の namespace の外へ逃がす。名前は導入の lock 内で
/// だけ使うので固定でよい。
fn backup(catalog: &ExtensionCatalog, id: &ExtensionPackageId) -> Result<(), PackageError> {
    let target = catalog.installed_directory(id)?;
    let previous = swap_path(catalog.root(), id);
    if let Some(parent) = previous.parent() {
        fs::create_dir_all(parent).map_err(PackageError::Io)?;
    }
    remove_directory(&previous)?;
    if target.is_dir() {
        fs::rename(&target, &previous).map_err(PackageError::Io)?;
    }
    Ok(())
}

/// 用意した一時 directory を導入済みの置き場所へ置く。
///
/// 先に [`backup`] で旧版を退避しておく。置けなかった場合は退避した旧版を元の場所へ
/// 戻し、失敗を返す。呼び出し側は journal の段階を使って同じ復元をやり直せる。
fn place(
    catalog: &ExtensionCatalog,
    id: &ExtensionPackageId,
    staged: &Path,
) -> Result<(), PackageError> {
    let target = catalog.installed_directory(id)?;
    let previous = swap_path(catalog.root(), id);
    if let Err(error) = fs::rename(staged, &target) {
        if previous.is_dir() {
            // 新しい実体を置けなかった。退避した旧版を元の場所へ戻す。
            if fs::rename(&previous, &target).is_err() {
                eprintln!(
                    "拡張 {id} の旧版を戻せませんでした。退避先は {} に残っています",
                    previous.display()
                );
            }
        }
        return Err(PackageError::Io(error));
    }
    Ok(())
}

fn swap_path(root: &Path, id: &ExtensionPackageId) -> PathBuf {
    crate::journal::swap_path(root, id)
}

/// 同じ package の同時導入を直列化する。回復と同じ lock を使い、導入中の操作を
/// 別の入口が中断済みと誤認しないようにする。
fn lock_install(root: &Path) -> Result<crate::journal::InstallLock, PackageError> {
    crate::journal::InstallLock::acquire(root).map_err(PackageError::Catalog)
}

fn validate_member_path(path: &str) -> Result<(), PackageError> {
    if path == MANIFEST_NAME {
        return Err(PackageError::Malformed(format!(
            "{MANIFEST_NAME} は package の予約名である"
        )));
    }
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.split('/').any(|part| {
            // Windows は path 要素の末尾の `.` と空白を取り除いて解決する。許可すると
            // `a.txt` と `a.txt.` が同じ file を指し、後から書いた内容で上書きされる。
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with('.')
                || part.ends_with(' ')
        })
    {
        return Err(PackageError::Malformed(format!("不正な path: {path}")));
    }
    Ok(())
}

fn valid_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn reject_unknown_and_missing(
    object: &serde_json::Map<String, Value>,
    fields: &[&str],
    label: &str,
) -> Result<(), PackageError> {
    if let Some(unknown) = object.keys().find(|key| !fields.contains(&key.as_str())) {
        return Err(PackageError::Malformed(format!(
            "{label} に未知の field: {unknown}"
        )));
    }
    if let Some(missing) = fields.iter().find(|field| !object.contains_key(**field)) {
        return Err(PackageError::Malformed(format!(
            "{label} に field が足りない: {missing}"
        )));
    }
    Ok(())
}
