use std::time::Duration;

use gpui::Context;
use process::{ProcessOrigin, ProcessRootId, ProcessSnapshot};

use crate::client_adapter::{ClientWorker, DocumentClient};

use super::{ResourceManagerFeature, TerminationFeedback};

impl ResourceManagerFeature {
    pub(crate) fn new(worker: ClientWorker<DocumentClient>, cx: &mut Context<Self>) -> Self {
        let mut feature = Self {
            worker,
            snapshot: ProcessSnapshot::default(),
            message: String::new(),
            terminate: TerminationFeedback::default(),
            pending_terminate: None,
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
                match result {
                    Ok((snapshot, host_error)) => {
                        feature.snapshot = snapshot;
                        feature.message = host_error.unwrap_or_default();
                        cx.notify();
                    }
                    Err(error) => {
                        feature.message = error;
                        cx.notify();
                    }
                }
                feature.schedule(cx);
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.message = error;
            cx.notify();
            self.schedule(cx);
        }
    }

    pub(crate) fn request_terminate(&mut self, root_id: ProcessRootId, cx: &mut Context<Self>) {
        self.pending_terminate = Some(root_id);
        self.terminate.begin();
        cx.notify();
    }

    pub(crate) fn cancel_terminate(&mut self, cx: &mut Context<Self>) {
        self.pending_terminate = None;
        self.terminate.begin();
        cx.notify();
    }

    pub(crate) fn confirm_terminate(&mut self, cx: &mut Context<Self>) {
        let Some(root_id) = self.pending_terminate.take() else {
            return;
        };
        let origin = self
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
                feature.terminate.finished(result);
                feature.refresh(cx);
            },
        );
        if let Err(error) = admitted {
            self.terminate.submitted(Err(error));
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TerminationFeedback;

    #[test]
    fn termination_feedback_is_not_cleared_by_a_successful_refresh() {
        let mut feedback = TerminationFeedback::default();
        feedback.submitted(Err("queue full".into()));
        // 定期再取得は refresh 用の message だけを更新し、この状態には触れない。
        assert_eq!(feedback.error(), Some("queue full"));
        feedback.finished(Err("termination failed".into()));
        assert_eq!(feedback.error(), Some("termination failed"));
        feedback.begin();
        assert_eq!(feedback.error(), None);
    }
}
