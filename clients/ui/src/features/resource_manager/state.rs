use std::time::Duration;

use gpui::Context;
use process::{ProcessOrigin, ProcessRootId};

use crate::client_adapter::{ClientWorker, DocumentClient};

use super::{ProcessPanelState, ResourceManagerFeature};

impl ResourceManagerFeature {
    pub(crate) fn new(worker: ClientWorker<DocumentClient>, cx: &mut Context<Self>) -> Self {
        let mut feature = Self {
            worker,
            state: ProcessPanelState::default(),
            busy: false,
            scheduled: false,
            refresh_generation: 0,
        };
        feature.refresh(cx);
        feature
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        if self.scheduled {
            return;
        }
        self.scheduled = true;
        cx.spawn(async move |this, cx| {
            crate::timer::Timer::after(Duration::from_secs(2)).await;
            let _ = this.update(cx, |feature, cx| {
                feature.scheduled = false;
                feature.refresh(cx);
            });
        })
        .detach();
    }

    /// Host と Desktop の現在値を取得する。1 回に 1 要求だけを直列化する。
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.schedule(cx);
            return;
        }
        self.busy = true;
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        let generation = self.refresh_generation;
        let admitted = self.worker.submit(
            cx,
            |client| {
                let workspace = client.editor.workspace_id().cloned();
                client
                    .process
                    .refresh(workspace.as_ref())
                    .map_err(|error| error.to_string())?;
                Ok((
                    client.process.snapshot().clone(),
                    client.process.host_error().map(ToOwned::to_owned),
                ))
            },
            move |feature, result, cx| {
                if generation != feature.refresh_generation {
                    feature.busy = false;
                    return;
                }
                feature.busy = false;
                feature.state.apply_refresh(result);
                cx.notify();
                feature.schedule(cx);
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.state.messages.refresh_failed(error);
            cx.notify();
            self.schedule(cx);
        }
    }

    pub(crate) fn request_terminate(&mut self, root_id: ProcessRootId, cx: &mut Context<Self>) {
        self.state.begin_termination(root_id);
        cx.notify();
    }

    pub(crate) fn cancel_terminate(&mut self, cx: &mut Context<Self>) {
        self.state.cancel_termination();
        cx.notify();
    }

    pub(crate) fn confirm_terminate(&mut self, cx: &mut Context<Self>) {
        let Some(root_id) = self.state.pending_terminate.take() else {
            return;
        };
        let origin = self
            .state
            .snapshot
            .roots
            .iter()
            .find(|root| root.root_id == root_id)
            .map(|root| root.origin)
            .unwrap_or(ProcessOrigin::Host);
        let admitted = self.worker.submit(
            cx,
            move |client| {
                let workspace = client.editor.workspace_id().cloned();
                client
                    .process
                    .terminate(workspace.as_ref(), origin, &root_id)
                    .map_err(|error| error.to_string())?;
                Ok(())
            },
            |feature, result, cx| {
                feature.state.termination_finished(result);
                feature.refresh(cx);
            },
        );
        if let Err(error) = admitted {
            self.state.termination_submitted(Err(error));
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use process::ProcessSnapshot;

    #[test]
    fn termination_feedback_is_not_cleared_by_a_successful_refresh() {
        let mut state = ProcessPanelState::default();
        state.termination_submitted(Err("queue full".into()));
        // 実際の再取得適用経路を通しても、終了エラーは消えない。
        state.apply_refresh(Ok((ProcessSnapshot::default(), None)));
        assert!(state.messages.refresh.is_empty());
        assert_eq!(state.messages.terminate.error(), Some("queue full"));
        assert_eq!(state.status_lines().1, Some("queue full"));

        state.termination_finished(Err("termination failed".into()));
        state.apply_refresh(Ok((
            ProcessSnapshot::default(),
            Some("host unavailable".into()),
        )));
        assert_eq!(state.messages.refresh, "host unavailable");
        assert_eq!(state.messages.terminate.error(), Some("termination failed"));
        // view が描画に使う値にも終了エラーが残る。
        assert_eq!(
            state.status_lines(),
            (Some("host unavailable"), Some("termination failed"))
        );

        state.cancel_termination();
        assert_eq!(state.messages.terminate.error(), None);
        assert_eq!(state.status_lines().1, None);
    }
}
