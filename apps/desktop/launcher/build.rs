//! Windows の exe に表示名とアイコンを埋め込み、埋め込んだ公開鍵 registry の変更を
//! 再ビルドへ伝える。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../../assets/lapis.ico");
    // `include_str!` の依存は cargo が自動で追跡しないため、明示する。
    println!("cargo:rerun-if-changed=../../../release/trusted-keys.json");
    embed(
        "Lapis Launcher",
        "lapis-launcher.exe",
        "../../../assets/lapis.ico",
    );
}

fn embed(description: &str, filename: &str, icon: &str) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
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
