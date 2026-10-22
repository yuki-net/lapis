use lapis_host::BackendClient;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

struct ManagedCleanup(PathBuf);
impl Drop for ManagedCleanup {
    fn drop(&mut self) {
        let Ok(bytes) = fs::read(self.0.join("local-v1.host")) else {
            return;
        };
        let Some(plaintext) = decrypt(&bytes) else {
            return;
        };
        #[derive(Deserialize)]
        struct Process {
            pid: u32,
        }
        let Ok(process) = serde_json::from_slice::<Process>(&plaintext) else {
            return;
        };
        if process.pid == 0 || process.pid == std::process::id() {
            return;
        }
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{
                Foundation::CloseHandle,
                System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess},
            };
            let handle = OpenProcess(PROCESS_TERMINATE, 0, process.pid);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(process.pid as libc::pid_t, libc::SIGTERM);
        }
    }
}
#[cfg(windows)]
fn decrypt(bytes: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let entropy = b"local-v1";
    let salt = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        if CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            &salt,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        ) == 0
        {
            return None;
        }
        let result = Zeroizing::new(
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec(),
        );
        use zeroize::Zeroize;
        std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize).zeroize();
        LocalFree(output.pbData.cast());
        Some(result)
    }
}
#[cfg(unix)]
fn decrypt(bytes: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    Some(Zeroizing::new(bytes.to_vec()))
}

fn backend(directory: &Path) -> BackendClient {
    BackendClient::local(
        PathBuf::from(env!("CARGO_BIN_EXE_lapis-host")),
        directory.to_owned(),
    )
}

#[test]
fn desktop_ports_reuse_one_independent_host_and_preserve_unsaved_documents_across_clients() {
    let state = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let _cleanup = ManagedCleanup(state.path().to_owned());
    fs::write(root.path().join("note.ts"), "const value = 1;").unwrap();
    let client = backend(state.path());
    let documents = client.documents();
    let workspace = documents
        .workspace(root.path())
        .expect("managed Host must bootstrap");
    let tree = client.workspaces().list_tree(root.path()).unwrap();
    assert!(tree.iter().any(|file| file.path.ends_with("note.ts")));
    let opened = documents.open(&root.path().join("note.ts")).unwrap();
    let edited = documents
        .edit(&opened.id, opened.document.revision(), 14..15, "42")
        .unwrap();
    assert!(edited.document.is_dirty());
    let record_before = fs::read(state.path().join("local-v1.host")).unwrap();
    #[cfg(windows)]
    assert_ne!(
        record_before.first(),
        Some(&b'{'),
        "record must be DPAPI-protected"
    );
    drop(documents);
    drop(client);
    let second = backend(state.path());
    let second_documents = second.documents();
    assert_eq!(second_documents.workspace(root.path()).unwrap(), workspace);
    let restored = second_documents.open(&root.path().join("note.ts")).unwrap();
    assert_eq!(restored.id, opened.id);
    assert!(restored.document.is_dirty());
    assert_eq!(
        restored.document.content_for_save(),
        edited.document.content_for_save()
    );
    assert_eq!(
        fs::read(state.path().join("local-v1.host")).unwrap(),
        record_before,
        "live Host must not be replaced on Client restart"
    );
    // 別rootも同じHostへ登録し、Save Asのidentityとundo履歴を維持する。
    let moved = second_documents
        .save(
            &restored.id,
            restored.document.revision(),
            Some(other.path().join("moved.ts")),
        )
        .unwrap();
    assert_eq!(moved.id, opened.id);
    assert_eq!(
        fs::read_to_string(other.path().join("moved.ts")).unwrap(),
        "const value = 42;"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("note.ts")).unwrap(),
        "const value = 1;"
    );
    assert!(moved.can_undo);
    assert_eq!(
        fs::read(state.path().join("local-v1.host")).unwrap(),
        record_before
    );
}

#[test]
fn managed_host_opens_verified_worktrees_and_shares_documents() {
    let state = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let _cleanup = ManagedCleanup(state.path().to_owned());
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    fs::write(root.path().join("note.ts"), "base").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-m", "base"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let client = backend(state.path());
    let documents = client.documents();
    let workspace = documents.workspace(root.path()).unwrap();
    let git = client.git();
    assert!(
        git.open_worktree(&workspace, &lapis_editor_core::TaskId::new("unknown"))
            .is_err()
    );
    let tree = git.create_worktree(&workspace).unwrap();
    let path = git.open_worktree(&workspace, &tree.worktree_id).unwrap();
    assert_ne!(path, root.path().canonicalize().unwrap());
    let opened = documents.open(&path.join("note.ts")).unwrap();
    documents
        .edit(&opened.id, opened.document.revision(), 0..4, "draft")
        .unwrap();
    let second = backend(state.path());
    let second_documents = second.documents();
    let second_workspace = second_documents.workspace(root.path()).unwrap();
    assert_eq!(
        second
            .git()
            .open_worktree(&second_workspace, &tree.worktree_id)
            .unwrap(),
        path
    );
    assert_eq!(
        second_documents
            .open(&path.join("note.ts"))
            .unwrap()
            .document
            .content_for_save(),
        "draft"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("note.ts")).unwrap(),
        "base"
    );
}

#[test]
fn corrupted_host_records_are_rejected_without_replacing_a_running_host() {
    let state = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let cleanup = ManagedCleanup(state.path().to_owned());
    let client = backend(state.path());
    client.documents().workspace(root.path()).unwrap();
    let path = state.path().join("local-v1.host");
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"corrupt data").unwrap();
    assert!(
        backend(state.path())
            .documents()
            .workspace(root.path())
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"corrupt data");
    // cleanupはこのテストが起動したHostだけを終了する。
    fs::write(path, original).unwrap();
    drop(cleanup);
}
