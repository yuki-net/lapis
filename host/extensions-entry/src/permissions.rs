//! Host 管理者の能力承認。宣言は署名済み manifest の内容に結び付ける。
//!
//! 呼び出し側は検証済み package を渡し、承認が無いか読めない場合は拒否する。

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{PackageCapability, VerifiedPackage};

const MAX_BYTES: u64 = 1024 * 1024;
const SCHEMA: u32 = 1;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: u32,
    approvals: Vec<Approval>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    extension: String,
    manifest_digest: String,
    capability: String,
    version: String,
}

/// Host のデータ領域内に承認を保存する。既定では一切承認しない。
pub struct CapabilityApprovals {
    root: PathBuf,
}

impl CapabilityApprovals {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// package が宣言した能力だけを取得する。不正な保存データはエラーとして拒否する。
    pub fn is_granted(
        &self,
        package: &VerifiedPackage,
        capability: &PackageCapability,
    ) -> io::Result<bool> {
        if !package.declares(capability) {
            return Ok(false);
        }
        let document = load(&self.root)?;
        Ok(document.approvals.contains(&approval(package, capability)))
    }

    /// 検証済み package に含まれる能力だけを承認・取り消す。
    pub fn set(
        &self,
        package: &VerifiedPackage,
        capability: &PackageCapability,
        granted: bool,
    ) -> io::Result<()> {
        if !package.declares(capability) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "宣言されていない能力です",
            ));
        }
        self.update(|document| {
            let record = approval(package, capability);
            document.approvals.retain(|entry| entry != &record);
            if granted {
                document.approvals.push(record);
            }
        })
    }

    /// package が読み込めない場合でも、特定能力の全バージョンの承認を取り消せる。
    pub fn revoke_capability(
        &self,
        id: &crate::ExtensionPackageId,
        capability: &crate::ContractId,
    ) -> io::Result<()> {
        self.update(|document| {
            document.approvals.retain(|entry| {
                entry.extension != id.as_str() || entry.capability != capability.as_str()
            })
        })
    }

    /// 再導入時に過去の承認を引き継がない。失敗時は削除処理を始めない。
    pub(crate) fn revoke_extension(&self, id: &crate::ExtensionPackageId) -> io::Result<()> {
        self.update(|document| {
            document
                .approvals
                .retain(|entry| entry.extension != id.as_str())
        })
    }

    fn update(&self, change: impl FnOnce(&mut Document)) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("capability-approvals.lock"))?;
        lock.lock()?;
        let mut document = load(&self.root)?;
        change(&mut document);
        document.approvals.sort_by(|a, b| {
            (&a.extension, &a.manifest_digest, &a.capability, &a.version).cmp(&(
                &b.extension,
                &b.manifest_digest,
                &b.capability,
                &b.version,
            ))
        });
        let bytes = serde_json::to_vec_pretty(&document).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "承認情報が大きすぎます",
            ));
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.root.join("capability-approvals.json"))
            .map_err(|e| e.error)?;
        Ok(())
    }
}

fn approval(package: &VerifiedPackage, capability: &PackageCapability) -> Approval {
    Approval {
        extension: package.verified_id().to_string(),
        manifest_digest: package.manifest_digest().to_owned(),
        capability: capability.id.to_string(),
        version: capability.version.to_string(),
    }
}

fn load(root: &Path) -> io::Result<Document> {
    let path = root.join("capability-approvals.json");
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Document {
                schema: SCHEMA,
                approvals: Vec::new(),
            });
        }
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "承認情報が大きすぎます",
        ));
    }
    let document: Document = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if document.schema != SCHEMA {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "未対応の承認情報です",
        ));
    }
    for a in &document.approvals {
        if crate::ExtensionPackageId::try_new(&a.extension).is_err()
            || crate::ContractId::try_new(&a.capability).is_err()
            || semver::Version::parse(&a.version).is_err()
            || a.manifest_digest.len() != 64
            || !a
                .manifest_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "承認情報が不正です",
            ));
        }
    }
    Ok(document)
}
