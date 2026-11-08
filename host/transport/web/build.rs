//! Windows の exe に表示名とアイコンを埋め込む。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../../assets/lapis.ico");

    embed("Lapis Web", "lapis-web.exe", "../../../assets/lapis.ico");
}

fn embed(description: &str, filename: &str, icon: &str) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // dx は自前でリソースをリンクするため、二重に埋め込まない。
    if std::env::var_os("DX_RUSTC").is_some() {
        return;
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let icon = root.join(icon);
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(&icon.to_string_lossy())
        .set("FileDescription", description)
        .set("ProductName", "Lapis")
        .set("OriginalFilename", filename);
    resource
        .compile()
        .expect("Windows リソースを埋め込めません");
}
