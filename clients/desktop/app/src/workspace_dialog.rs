use std::path::PathBuf;

use lapis_desktop_client::WorkspaceDialog;

#[derive(Default)]
pub(super) struct NativeWorkspaceDialog;

pub(super) struct RemoteWorkspaceDialog(pub PathBuf);
impl WorkspaceDialog for RemoteWorkspaceDialog {
    fn path_scope(&self) -> Option<PathBuf> {
        Some(self.0.clone())
    }
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        Some(self.0.clone())
    }
    fn choose_save_path(&self, _: &str) -> Option<PathBuf> {
        None
    }
}

impl WorkspaceDialog for NativeWorkspaceDialog {
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        rfd::FileDialog::new().pick_folder()
    }

    fn choose_file_path(&self) -> Option<PathBuf> {
        rfd::FileDialog::new().pick_file()
    }

    fn choose_save_path(&self, suggested_name: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_file_name(suggested_name)
            .save_file()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn native_dialog_can_be_injected_through_the_workspace_dialog_contract() {
        let _dialog: Arc<dyn WorkspaceDialog> = Arc::new(NativeWorkspaceDialog);
    }
}
