use std::time::Duration;

use gpui::Context;
use process::{ProcessOrigin, ProcessRootId, ProcessSnapshot};

use crate::client_adapter::{ClientWorker, DocumentClient};

use super::{ResourceManagerFeature, ResourceMessages};

impl ResourceManagerFeature {
    pub(crate) fn new(worker: ClientWorker<DocumentClient>, cx: &mut Context<Self>) -> Self {
        let mut feature = Self {
            worker,
            snapshot: ProcessSnapshot::default(),
            messages: ResourceMessages::default(),
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
                        feature.messages.refresh_succeeded(host_error);
                        cx.notify();
                    }
                    Err(error) => {
                        feature.messages.refresh_failed(error);
                        cx.notify();
                    }
                }
                feature.schedule(cx);
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.messages.refresh_failed(error);
            cx.notify();
            self.schedule(cx);
        }
    }

    pub(crate) fn request_terminate(&mut self, root_id: ProcessRootId, cx: &mut Context<Self>) {
        self.pending_terminate = Some(root_id);
        self.messages.terminate.begin();
        cx.notify();
    }

    pub(crate) fn cancel_terminate(&mut self, cx: &mut Context<Self>) {
        self.pending_terminate = None;
        self.messages.terminate.begin();
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
                feature.messages.terminate.finished(result);
                feature.refresh(cx);
            },
        );
        if let Err(error) = admitted {
            self.messages.terminate.submitted(Err(error));
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ResourceMessages;

    #[test]
    fn termination_feedback_is_not_cleared_by_a_successful_refresh() {
        let mut messages = ResourceMessages::default();
        messages.terminate.submitted(Err("queue full".into()));
        // 成功した再取得は refresh 側だけを更新し、終了エラーを消さない。
        messages.refresh_succeeded(None);
        assert!(messages.refresh.is_empty());
        assert_eq!(messages.terminate.error(), Some("queue full"));

        messages
            .terminate
            .finished(Err("termination failed".into()));
        messages.refresh_succeeded(Some("host unavailable".into()));
        assert_eq!(messages.refresh, "host unavailable");
        assert_eq!(messages.terminate.error(), Some("termination failed"));

        messages.terminate.begin();
        assert_eq!(messages.terminate.error(), None);
    }
}
