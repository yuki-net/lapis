wit_bindgen::generate!({ path: "wit", world: "extension", generate_all });

use lapis::pty::sessions::{self, Size, SpawnRequest};

struct Example;

impl Guest for Example {
    fn activate() {
        // 同一 component を Windows と Unix の両方で検証する。
        let commands: &[(&str, &[&str])] = &[("cmd.exe", &["/D", "/C", "ping -n 60 127.0.0.1"]), ("sh", &["-c", "sleep 60"])];
        for (program, args) in commands {
            let command = SpawnRequest {
                program: (*program).to_owned(),
                arguments: args.iter().map(|arg| (*arg).to_owned()).collect(),
                environment: vec![],
                working_directory: ".".to_owned(),
                initial_size: Size { columns: 80, rows: 24 },
            };
            match sessions::spawn(&command) {
                Ok(id) => {
                    assert!(!cfg!(feature = "expect-denied"));
                    sessions::attach(&id).unwrap();
                    // 終了操作を Host のクラッシュ回復検証に委ねる。
                    return;
                }
                Err(_) if cfg!(feature = "expect-denied") => return,
                Err(_) => continue,
            }
        }
        panic!("no PTY command could be started");
    }

    fn deactivate() {
        let _ = sessions::attach("pty-1");
    }
}

export!(Example);
