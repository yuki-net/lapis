//! Desktop から Local Host の Web 公開と端末を管理する port。
use std::path::Path;

pub use client_api::{HostDevice, HostWebStatus};

/// すべての操作はブロッキングし得るため、UI worker から呼び出す。
pub trait WebAccessControl: Send + Sync {
    fn status(&self, root: &Path) -> Result<HostWebStatus, String>;
    fn start(&self, root: &Path, address: &str, port: u16) -> Result<HostWebStatus, String>;
    fn stop(&self, root: &Path) -> Result<HostWebStatus, String>;
    fn invite(&self, root: &Path) -> Result<zeroize::Zeroizing<String>, String>;
    fn devices(&self, root: &Path) -> Result<Vec<HostDevice>, String>;
    fn revoke(&self, root: &Path, device_id: &str) -> Result<(), String>;
}
