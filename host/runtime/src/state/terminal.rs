//! Terminal の request 処理。

use super::*;

impl BackendState {
    pub(super) fn terminal_list_profiles(
        &mut self,
        session: &BackendSession,
        request: TerminalListProfilesRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        self.require_workspace(session, &request.workspace_id)?;
        self.require_connected(session)?;
        let workspace = self.workspace(session)?;
        let profiles = workspace.terminals.profiles(workspace.resolver.root())?;
        Ok((
            ResponseBody::TerminalListProfiles(TerminalListProfilesResponse { profiles }),
            Vec::new(),
        ))
    }

    pub(super) fn terminal_start(
        &mut self,
        session: &BackendSession,
        request: TerminalStartRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        if request.command.is_some() {
            return Err(BackendStateError::Unsupported);
        }
        let terminal_id = self.allocate_terminal_id()?;
        let workspace = self.workspace_mut(session)?;
        let cwd = workspace.resolver.resolve_directory(request.cwd.as_ref())?;
        let (terminal, event_body) = workspace.terminals.start(
            terminal_id,
            &session.session_id,
            &workspace.summary.workspace_id,
            &cwd,
            &request,
        )?;
        let event = workspace.publish(event_body)?;
        Ok((
            ResponseBody::TerminalStart(TerminalStartResponse { terminal }),
            vec![event],
        ))
    }

    pub(super) fn terminal_input(
        &mut self,
        session: &BackendSession,
        request: TerminalInputRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        workspace.terminals.input(
            &session.session_id,
            &workspace.summary.workspace_id,
            &request.terminal_id,
            &request.input,
        )?;
        Ok((
            ResponseBody::TerminalInput(TerminalCommandResponse::default()),
            Vec::new(),
        ))
    }

    pub(super) fn terminal_resize(
        &mut self,
        session: &BackendSession,
        request: TerminalResizeRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let (_, event_body) = workspace.terminals.resize(
            &session.session_id,
            &workspace.summary.workspace_id,
            &request.terminal_id,
            request.size,
        )?;
        let event = workspace.publish(event_body)?;
        Ok((
            ResponseBody::TerminalResize(TerminalCommandResponse::default()),
            vec![event],
        ))
    }

    pub(super) fn terminal_terminate(
        &mut self,
        session: &BackendSession,
        request: TerminalTerminateRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let (_, event_body) = workspace.terminals.terminate(
            &session.session_id,
            &workspace.summary.workspace_id,
            &request.terminal_id,
        )?;
        let events = event_body
            .map(|event| workspace.publish(event))
            .transpose()?
            .into_iter()
            .collect();
        Ok((
            ResponseBody::TerminalTerminate(TerminalCommandResponse::default()),
            events,
        ))
    }

    pub(super) fn terminal_claim(
        &mut self,
        session: &BackendSession,
        request: TerminalClaimRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let (_, event_body) = workspace.terminals.claim(
            &session.session_id,
            &workspace.summary.workspace_id,
            &request.terminal_id,
        )?;
        let event = workspace.publish(event_body)?;
        Ok((
            ResponseBody::TerminalClaim(TerminalCommandResponse::default()),
            vec![event],
        ))
    }

    pub(super) fn terminal_release(
        &mut self,
        session: &BackendSession,
        request: TerminalReleaseRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let (_, event_body) = workspace.terminals.release(
            &session.session_id,
            &workspace.summary.workspace_id,
            &request.terminal_id,
        )?;
        let event = workspace.publish(event_body)?;
        Ok((
            ResponseBody::TerminalRelease(TerminalCommandResponse::default()),
            vec![event],
        ))
    }
}
