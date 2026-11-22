//! Shell / Root が共有する window 部品。ネイティブ起動 entry は `apps/desktop/app` が持つ。

pub(crate) mod window_host;
mod window_options;

pub(crate) use window_options::{window_options, window_options_with_title};
