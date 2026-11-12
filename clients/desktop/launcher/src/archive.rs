//! package (zip) の展開と、展開済み file の照合。
//!
//! layout の規則は `scripts/release/layout.py` と同じものを使う。zip member の絶対パス、
//! `..`、drive 指定、`\`、layout の予約名を拒否する。

use std::collections::BTreeMap;
use std::path::Path;

use zip::read::ZipFile;

use crate::manifest::sha256_bytes;
use crate::{Error, Result};

/// layout が root に置くため、package に含めてはいけない名前。
pub(crate) const RESERVED_ROOT_NAMES: [&str; 2] = ["current.json", "manifest.json"];

/// zip の root を `destination` の直下へ展開する。
///
/// `reserved` には manifest や package 自身の file 名を渡す。展開がそれらを上書きすると、
/// 保存済み manifest / ZIP と展開内容の照合が自分の書き換えを追認してしまう。
pub(crate) fn extract(package: &Path, destination: &Path, reserved: &[&str]) -> Result<()> {
    let file = std::fs::File::open(package).map_err(|error| {
        Error::new(format!(
            "package を開けません ({}): {error}",
            package.display()
        ))
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        Error::new(format!(
            "package が zip でない ({}): {error}",
            package.display()
        ))
    })?;
    let mut seen = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::new(format!("package の member を読めません: {error}")))?;
        let name = member_name(&entry)?;
        if entry.is_dir() {
            continue;
        }
        if entry.is_symlink() {
            return Err(Error::new(format!("package に symlink がある: {name}")));
        }
        if reserved.contains(&name.as_str()) {
            return Err(Error::new(format!(
                "package が layout の保持 file を上書きする: {name}"
            )));
        }
        if seen.insert(name.clone(), ()).is_some() {
            return Err(Error::new(format!(
                "package に重複した path がある: {name}"
            )));
        }
        let target = destination.join(&name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::new(format!(
                    "展開先を作れません ({}): {error}",
                    parent.display()
                ))
            })?;
        }
        let mut sink = std::fs::File::create(&target).map_err(|error| {
            Error::new(format!(
                "展開先を書けません ({}): {error}",
                target.display()
            ))
        })?;
        std::io::copy(&mut entry, &mut sink)
            .map_err(|error| Error::new(format!("package を展開できません: {error}")))?;
    }
    Ok(())
}

/// entry の名前を検証して返す。directory entry の末尾 `/` は除く。
fn member_name(entry: &ZipFile<'_, std::fs::File>) -> Result<String> {
    let raw = std::str::from_utf8(entry.name_raw())
        .map_err(|_| Error::new("package の member 名が UTF-8 でない"))?;
    check_member_name(raw)?;
    let name = raw.trim_end_matches('/');
    let root = name.split('/').next().unwrap_or_default();
    if RESERVED_ROOT_NAMES.contains(&root) {
        return Err(Error::new(format!(
            "package が layout の予約名を使っている: {name}"
        )));
    }
    Ok(name.to_owned())
}

fn check_member_name(raw: &str) -> Result<()> {
    let name = raw.trim_end_matches('/');
    if name.is_empty()
        || name.starts_with('/')
        || raw.contains('\\')
        || name.contains(':')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Error::new(format!("package に不正な path がある: {raw}")));
    }
    Ok(())
}

/// `directory` 配下の展開済み file を package の内容と照合し、不一致を返す。
///
/// `skip` には manifest や ZIP 自身など、展開内容に含まれない file 名を渡す。
pub(crate) fn mismatches(directory: &Path, package: &Path, skip: &[&str]) -> Result<Vec<String>> {
    let expected = package_hashes(package)?;
    let actual = directory_hashes(directory, skip)?;
    let mut problems = Vec::new();
    for name in expected.keys() {
        match actual.get(name) {
            None => problems.push(format!("{name} が無い")),
            Some(None) => problems.push(format!("{name} が symlink")),
            Some(Some(digest)) if Some(digest) == expected.get(name) => {}
            Some(Some(_)) => problems.push(format!("{name} が package と違う")),
        }
    }
    for name in actual.keys() {
        if !expected.contains_key(name) {
            problems.push(format!("{name} が package に無い"));
        }
    }
    Ok(problems)
}

fn package_hashes(package: &Path) -> Result<BTreeMap<String, String>> {
    let file = std::fs::File::open(package).map_err(|error| {
        Error::new(format!(
            "package を開けません ({}): {error}",
            package.display()
        ))
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        Error::new(format!(
            "package が zip でない ({}): {error}",
            package.display()
        ))
    })?;
    let mut hashes = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::new(format!("package の member を読めません: {error}")))?;
        let name = member_name(&entry)?;
        if entry.is_dir() {
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)
            .map_err(|error| Error::new(format!("package を読めません: {error}")))?;
        hashes.insert(name, sha256_bytes(&bytes));
    }
    Ok(hashes)
}

/// 展開済み directory の file の SHA-256。symlink は `None` として不一致にする。
fn directory_hashes(directory: &Path, skip: &[&str]) -> Result<BTreeMap<String, Option<String>>> {
    let mut hashes = BTreeMap::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = std::fs::read_dir(&current)
            .map_err(|error| Error::new(format!("{} を読めません: {error}", current.display())))?;
        for entry in entries {
            let entry = entry.map_err(|error| Error::new(format!("{error}")))?;
            let path = entry.path();
            let relative = path
                .strip_prefix(directory)
                .map_err(|_| Error::new("展開先の相対パスを解決できません"))?
                .to_string_lossy()
                .replace('\\', "/");
            if skip.contains(&relative.as_str()) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                Error::new(format!("{} を調べられません: {error}", path.display()))
            })?;
            if metadata.is_symlink() {
                hashes.insert(relative, None);
            } else if metadata.is_dir() {
                pending.push(path);
            } else {
                hashes.insert(relative, Some(crate::manifest::sha256_file(&path)?));
            }
        }
    }
    Ok(hashes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().expect("temp");
        let path = directory.path().join("package.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).expect("create"));
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .expect("start");
        writer.write_all(b"contents").expect("write");
        writer.finish().expect("finish");
        (directory, path)
    }

    #[test]
    fn members_cannot_escape_or_reuse_layout_names() {
        for name in [
            "../evil.txt",
            "a/../../evil.txt",
            "/abs.txt",
            "C:/abs.txt",
            "a\\b.txt",
            "current.json",
            "manifest.json",
        ] {
            let (directory, package) = zip_with(name);
            let destination = directory.path().join("out");
            std::fs::create_dir(&destination).expect("mkdir");
            assert!(extract(&package, &destination, &[]).is_err(), "{name}");
        }
    }

    #[test]
    fn extraction_and_comparison_detect_changes() {
        let (directory, package) = zip_with("lapis.exe");
        let destination = directory.path().join("out");
        std::fs::create_dir(&destination).expect("mkdir");
        extract(&package, &destination, &[]).expect("extract");
        assert!(
            mismatches(&destination, &package, &[])
                .expect("compare")
                .is_empty()
        );
        std::fs::write(destination.join("lapis.exe"), b"tampered").expect("tamper");
        assert!(
            !mismatches(&destination, &package, &[])
                .expect("compare")
                .is_empty()
        );
        std::fs::write(destination.join("extra.txt"), b"extra").expect("extra");
        assert!(
            !mismatches(&destination, &package, &[])
                .expect("compare")
                .is_empty()
        );
    }

    #[test]
    fn extraction_refuses_to_overwrite_a_reserved_file() {
        let (directory, package) = zip_with("package.zip");
        let destination = directory.path().join("out");
        std::fs::create_dir(&destination).expect("mkdir");
        std::fs::write(destination.join("package.zip"), b"original").expect("reserved");
        assert!(extract(&package, &destination, &["package.zip"]).is_err());
        assert_eq!(
            std::fs::read(destination.join("package.zip")).expect("reserved"),
            b"original"
        );
    }

    #[test]
    fn skipped_names_are_not_compared() {
        let (directory, package) = zip_with("lapis.exe");
        let destination = directory.path().join("out");
        std::fs::create_dir(&destination).expect("mkdir");
        extract(&package, &destination, &[]).expect("extract");
        std::fs::write(destination.join("manifest.json"), b"{}").expect("manifest");
        assert!(
            !mismatches(&destination, &package, &[])
                .expect("compare")
                .is_empty()
        );
        assert!(
            mismatches(&destination, &package, &["manifest.json"])
                .expect("compare")
                .is_empty()
        );
    }
}
