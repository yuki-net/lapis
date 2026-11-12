use std::path::Path;

use desktop_client::web_access::{HostDevice, HostWebStatus, WebAccess};

pub(super) struct LocalWebAccess(pub host::BackendClient);

impl LocalWebAccess {
    fn workspace(&self, root: &Path) -> Result<client_api::WorkspaceId, String> {
        self.0
            .register_workspace(root)
            .map_err(|error| error.to_string())
    }
}

impl WebAccess for LocalWebAccess {
    fn status(&self, root: &Path) -> Result<HostWebStatus, String> {
        self.0
            .web_status(self.workspace(root)?)
            .map_err(|error| error.to_string())
    }

    fn start(&self, root: &Path, address: &str, port: u16) -> Result<HostWebStatus, String> {
        self.0
            .start_web(self.workspace(root)?, address.to_owned(), port)
            .map_err(|error| error.to_string())
    }

    fn stop(&self, root: &Path) -> Result<HostWebStatus, String> {
        self.0
            .stop_web(self.workspace(root)?)
            .map_err(|error| error.to_string())
    }

    fn invite(&self, root: &Path) -> Result<zeroize::Zeroizing<String>, String> {
        let workspace = self.workspace(root)?;
        let status = self
            .0
            .web_status(workspace.clone())
            .map_err(|error| error.to_string())?;
        let url = status.url.ok_or("Web 公開を開始してください")?;
        let token = self
            .0
            .issue_web_invitation(workspace)
            .map_err(|error| error.to_string())?;
        Ok(zeroize::Zeroizing::new(format!(
            "{}#/invite?token={}",
            url,
            token.as_str()
        )))
    }

    fn devices(&self, root: &Path) -> Result<Vec<HostDevice>, String> {
        self.0
            .managed_devices(self.workspace(root)?)
            .map_err(|error| error.to_string())
    }

    fn revoke(&self, root: &Path, device_id: &str) -> Result<(), String> {
        match self
            .0
            .revoke_managed_device(self.workspace(root)?, device_id.to_owned())
        {
            Ok(true) => Ok(()),
            Ok(false) => Err("端末が見つかりません".into()),
            Err(error) => Err(error.to_string()),
        }
    }
}
