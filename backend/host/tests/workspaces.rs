use lapis_client_api::{FileTreeRequest, RequestBody, ResponseBody};
use lapis_host::BackendHost;
use lapis_workspace::FileEntryKind;

#[test]
fn desktop_and_wire_directory_creation_share_runtime_scope_and_tree_updates() {
    let root = tempfile::tempdir().unwrap();
    let foreign = tempfile::tempdir().unwrap();
    let host = BackendHost::start().unwrap();
    let client = host.connect(root.path()).unwrap();
    let subscription = client.subscribe().unwrap();
    host.workspaces()
        .create_directory(root.path(), "desktop-folder")
        .unwrap();
    assert!(root.path().join("desktop-folder").is_dir());
    assert!(
        subscription
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok()
    );
    let path = lapis_client_api::WorkspaceRelativePath::parse("wire-folder").unwrap();
    assert!(matches!(
        client
            .request(RequestBody::CreateDirectory(
                lapis_client_api::CreateDirectoryRequest {
                    workspace_id: client.workspace_id().clone(),
                    path: path.clone(),
                }
            ))
            .unwrap(),
        ResponseBody::CreateDirectory(_)
    ));
    assert!(root.path().join("wire-folder").is_dir());
    assert!(
        host.workspaces()
            .create_directory(root.path(), "../escaped")
            .is_err()
    );
    let other = host.register_workspace(foreign.path()).unwrap();
    assert!(matches!(
        client
            .request(RequestBody::CreateDirectory(
                lapis_client_api::CreateDirectoryRequest {
                    workspace_id: other,
                    path,
                }
            ))
            .unwrap(),
        ResponseBody::Error(_)
    ));
    assert!(!foreign.path().join("wire-folder").exists());
    assert_eq!(host.workspaces().list_tree(root.path()).unwrap().len(), 2);
}

#[test]
fn desktop_tree_uses_the_same_workspace_and_file_contract_as_wire_clients() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::create_dir(root.path().join("target")).unwrap();
    std::fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(root.path().join("target/generated"), "skip").unwrap();
    std::fs::write(root.path().join("note.md"), "note").unwrap();
    let host = BackendHost::start().unwrap();
    let desktop = host.workspaces();
    let tree = desktop.list_tree(root.path()).unwrap();
    assert_eq!(tree.len(), 3);
    assert_eq!(tree[0].relative_path, std::path::Path::new("src"));
    assert_eq!(tree[0].kind, FileEntryKind::Directory);
    assert_eq!(tree[1].relative_path, std::path::Path::new("src/main.rs"));
    assert_eq!(tree[1].depth, 1);
    assert_eq!(tree[2].relative_path, std::path::Path::new("note.md"));
    let client = host.connect(root.path()).unwrap();
    let ResponseBody::FileTree(response) = client
        .request(RequestBody::FileTree(FileTreeRequest {
            workspace_id: client.workspace_id().clone(),
            path: None,
        }))
        .unwrap()
    else {
        panic!("tree response required");
    };
    assert_eq!(response.workspace_id, *client.workspace_id());
    assert!(
        response
            .entries
            .iter()
            .any(|entry| entry.path.as_str() == "src")
    );
    assert_eq!(
        host.register_workspace(root.path()).unwrap(),
        *client.workspace_id()
    );
}
