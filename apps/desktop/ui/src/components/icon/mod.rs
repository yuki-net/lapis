mod assets;
mod catalog;
mod element;

pub(crate) use assets::IconAssets;
pub(crate) use catalog::IconName;
#[allow(unused_imports)] // 既存画面の置換は行わず、段階的に利用する。
pub(crate) use element::Icon;
