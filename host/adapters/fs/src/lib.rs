//! 複数の Local adapter が共有するファイルシステム操作。

use std::{fs, io, path::Path};

use serde::Serialize;

/// JSON を同一ディレクトリの一時ファイル経由で原子的に置き換える。
pub fn atomic_json_write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, value)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
