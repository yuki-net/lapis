//! UI 文言の解決と言語パックの検証を担う。UI・設定保存・取得元には依存しない。

mod bundled;
mod ids;
mod localizer;
mod pack;

pub use ids::{LocaleId, MessageId};
pub use localizer::Localizer;
pub use pack::{LocalePack, LocalePackError, LocalePackManifest};
