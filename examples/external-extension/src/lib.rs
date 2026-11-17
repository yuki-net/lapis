wit_bindgen::generate!({
    path: "wit",
    world: "extension",
});

struct Example;

impl Guest for Example {
    fn activate() {
        // 既定では実行元の環境変数やファイルシステムを継承しない。
        assert!(std::env::vars_os().next().is_none());
        assert!(std::fs::read_dir("/").is_err());
        assert!(std::net::TcpListener::bind("127.0.0.1:0").is_err());
    }
    fn deactivate() {}

    fn dispatch(operation: String, payload: String) -> Result<String, String> {
        match operation.as_str() {
            "echo" => Ok(payload),
            _ => Err("unknown operation".into()),
        }
    }
}

export!(Example);
