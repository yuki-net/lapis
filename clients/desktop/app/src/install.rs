//! per-user install layout の検出と、version ごとの data 世代の解決。
//!
//! launcher は `versions/<version>/lapis.exe` を起動する。Desktop は自分の実行 path から
//! install root を求め、`data/<generation>/` と `credentials/` を使う。layout の契約は
//! [`lapis_launcher`] と `docs/specs/release.md` を正とする。
//!
//! install root を検出できない場合（開発用の `target/debug/lapis.exe` など）は `None` を
//! 返し、従来の state directory をそのまま使う。

use std::path::{Component, Path, PathBuf};

/// 検出した install layout。
pub(crate) struct InstallLayout {
    root: PathBuf,
    version: Option<String>,
    generation: u32,
}

impl InstallLayout {
    /// `<install root>/versions/<version>/lapis.exe` の配置から layout を求める。
    ///
    /// 環境変数 `LAPIS_INSTALL_ROOT` があればそれを優先する。version は実行 path の
    /// 親 directory 名から決め、`current.json` とは独立に読む（旧版の起動でも正しい
    /// 世代を使うため）。
    pub(crate) fn detect() -> Option<Self> {
        let executable = std::env::current_exe().ok()?;
        let version_directory = executable.parent()?;
        if version_directory.parent()?.file_name()? != "versions" {
            return None;
        }
        let root = std::env::var_os("LAPIS_INSTALL_ROOT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                version_directory
                    .parent()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .unwrap_or_default()
            });
        let root = normalize(&root)?;
        if !root.join("versions").is_dir() {
            return None;
        }
        let version = version_directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        Self::open(root, version)
    }

    fn open(root: PathBuf, version: Option<String>) -> Option<Self> {
        // 世代の記録は version ごとに残る。記録が無い version は現在の世代を使う。
        let generation = version
            .as_deref()
            .and_then(|version| {
                lapis_launcher::generation_for(&root, version)
                    .ok()
                    .flatten()
            })
            .or_else(|| lapis_launcher::read_generation(&root).ok())
            .unwrap_or(1);
        Some(Self {
            root,
            version,
            generation,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// この version が使う data 世代の directory。
    pub(crate) fn data_directory(&self) -> PathBuf {
        lapis_launcher::data_dir(&self.root).join(self.generation.to_string())
    }

    /// data 世代の外に置く credential の directory。
    pub(crate) fn credentials_directory(&self) -> PathBuf {
        lapis_launcher::credentials_dir(&self.root)
    }

    /// 現在の世代の data directory をこの version のものとして確定する。
    ///
    /// 軸A の起動では何も変えない。軸B の同意後に再起動したとき、起動した version が
    /// 新しい世代を使ったことを記録する（`data/versions.json`）。
    pub(crate) fn record(&self) {
        let Some(version) = self.version.as_deref() else {
            return;
        };
        if let Err(error) = lapis_launcher::record_generation(&self.root, version, self.generation)
        {
            eprintln!("data 世代を記録できません: {error}");
        }
    }

    /// 世代の切り替えを確定する。起動中の version と世代の対応も残す。
    #[allow(dead_code)] // 同意後の再起動で launcher が参照する値をテストから確かめる。
    pub(crate) fn apply_generation(&self, generation: u32) -> Result<(), lapis_launcher::Error> {
        lapis_launcher::write_generation(&self.root, generation)?;
        if let Some(version) = self.version.as_deref() {
            lapis_launcher::record_generation(&self.root, version, generation)?;
        }
        Ok(())
    }
}

/// `..` を含まない絶対 path だけを受け付ける。
fn normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return None;
    }
    Some(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_absolute_root_is_not_a_managed_install() {
        assert!(normalize(Path::new("relative/root")).is_none());
        assert!(normalize(Path::new("root/../escape")).is_none());
        let absolute = std::env::temp_dir().join("lapis-install");
        assert!(normalize(&absolute).is_some());
    }

    #[test]
    fn the_generation_comes_from_the_version_record() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(root.path().join("versions/0.1.0")).expect("versions");
        std::fs::create_dir_all(lapis_launcher::data_dir(root.path()).join("1")).expect("data");
        std::fs::create_dir_all(lapis_launcher::data_dir(root.path()).join("2")).expect("data");
        lapis_launcher::write_generation(root.path(), 2).expect("pointer");
        lapis_launcher::record_generation(root.path(), "0.1.0", 1).expect("record");

        let old =
            InstallLayout::open(root.path().to_owned(), Some("0.1.0".into())).expect("layout");
        assert_eq!(old.data_directory(), root.path().join("data/1"));

        // 記録の無い version は現在の世代を使う。
        let new =
            InstallLayout::open(root.path().to_owned(), Some("0.2.0".into())).expect("layout");
        assert_eq!(new.data_directory(), root.path().join("data/2"));
    }

    #[test]
    fn credentials_are_outside_the_data_generation() {
        let root = tempfile::tempdir().expect("temp");
        let layout = InstallLayout::open(root.path().to_owned(), None).expect("layout");
        assert_eq!(
            layout.credentials_directory(),
            root.path().join("credentials")
        );
        assert!(
            !layout
                .credentials_directory()
                .starts_with(lapis_launcher::data_dir(root.path()))
        );
    }

    #[test]
    fn recording_keeps_the_version_bound_to_its_generation() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(lapis_launcher::data_dir(root.path()).join("1")).expect("data");
        let layout =
            InstallLayout::open(root.path().to_owned(), Some("0.1.0".into())).expect("layout");
        layout.record();
        assert_eq!(
            lapis_launcher::generation_for(root.path(), "0.1.0").expect("read"),
            Some(1)
        );
    }

    #[test]
    fn switching_the_generation_keeps_the_old_one_usable() {
        let root = tempfile::tempdir().expect("temp");
        let old = lapis_launcher::data_dir(root.path()).join("1");
        std::fs::create_dir_all(&old).expect("data");
        std::fs::write(old.join("session"), "old data").expect("data");
        std::fs::create_dir_all(lapis_launcher::data_dir(root.path()).join("2")).expect("data");
        let layout =
            InstallLayout::open(root.path().to_owned(), Some("0.3.0".into())).expect("layout");
        layout.apply_generation(2).expect("switch");

        assert_eq!(
            lapis_launcher::read_generation(root.path()).expect("read"),
            2
        );
        // 旧世代の data は残り、旧 version は旧世代で再開できる。
        assert_eq!(
            std::fs::read_to_string(old.join("session")).expect("old data"),
            "old data"
        );
        assert_eq!(
            lapis_launcher::generation_for(root.path(), "0.3.0").expect("read"),
            Some(2)
        );
    }
}
