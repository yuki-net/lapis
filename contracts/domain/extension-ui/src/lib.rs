//! 拡張が公開する UI 宣言の語彙。
//!
//! 配置・表示名・アイコン・表示条件などの宣言情報だけを型として持つ。GPUI の
//! View・Entity の生成、宣言の解決、Feature の状態管理は Client 側の内部
//! レジストリが所有し、この crate へ持ち込まない。

mod activation;
mod contribution;
mod feature;
mod ids;

pub use activation::ActivationCondition;
pub use contribution::{
    PanelScrollPolicy, ScrollAxis, ToolInstancePolicy, UiContribution, UiSlot,
    ViewPlacementCapabilities, ViewVisibility, ZonePosition,
};
pub use feature::{FeatureDescriptor, FeatureKind};
pub use ids::{
    CommandId, FeatureId, IconId, IconThemeId, KeymapId, ThemeId, ViewId, ViewInstanceId,
    WorkspaceCapabilityId,
};
pub use localization::MessageId;
