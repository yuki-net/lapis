//! 宣言から実際の GPUI View を配置する内部レジストリ。
//!
//! 宣言の型（配置・表示名・アイコン・表示条件）は `extension-ui-contract` が正本で、
//! このモジュールは宣言を消費する側として GPUI の Entity・表示状態・配置の解決を
//! 所有する。契約層へ GPUI の型や状態を持ち込まない。
//!
//! 宣言の型はここから再エクスポートしない。利用側は `extension_ui_contract` を直接参照する。

mod activation;
mod registry;

pub use activation::ActivationEvent;
pub use registry::{FeatureRegistry, FeatureState};

pub(crate) use registry::{ViewLifecycle, ViewNotifier, ViewRequest, shared_tool_instance};
