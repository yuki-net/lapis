use lapis_client_api::{FileTreeRequest, RequestBody, ResponseBody};
use lapis_host::BackendHost;
use lapis_workspace::FileEntryKind;

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
