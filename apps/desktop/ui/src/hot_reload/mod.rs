mod definition;
mod registry;
mod runtime;

pub(crate) use runtime::HotReloadDemo;

#[cfg(debug_assertions)]
pub(crate) fn definition_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("hot-reload")
        .join("demo.toml")
}
