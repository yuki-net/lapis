//! Host の同一性（証明書・鍵の保存と更新）と OS に依存する保存データの保護。

mod identity;
mod protected;

pub use identity::{Error, HostIdentity, certificate_lifetime};
pub use protected::protect;
