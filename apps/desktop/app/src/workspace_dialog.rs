use std::path::PathBuf;

use lapis_app_services::WorkspaceDialog;

#[derive(Default)]
pub(super) struct NativeWorkspaceDialog;

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
