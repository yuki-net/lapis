use crate::{client_adapter::ClientWorker, commands::UiCommand};
use desktop_client::WorkspaceSearchSession;
use gpui::Context;
use std::path::PathBuf;
use workspace::SearchHit;

pub(crate) struct SearchFeature {
    worker: ClientWorker<WorkspaceSearchSession>,
    pub(super) root: Option<PathBuf>,
    pub(super) query: String,
    pub(super) hits: Vec<SearchHit>,
    pub(super) message: String,
    pub(super) running: bool,
    generation: u64,
}
impl gpui::EventEmitter<UiCommand> for SearchFeature {}
impl SearchFeature {
    pub(crate) fn new(session: WorkspaceSearchSession, _cx: &mut Context<Self>) -> Self {
        Self {
            worker: ClientWorker::new(session),
            root: None,
            query: String::new(),
            hits: Vec::new(),
            message: String::new(),
            running: false,
            generation: 0,
        }
    }
    pub(crate) fn set_workspace(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.generation += 1;
        self.root = root;
        self.query.clear();
        self.hits.clear();
        self.running = false;
        if let Err(error) = self.worker.try_request(|session| {
            session.cancel();
            Ok(())
        }) {
            self.message = error;
        }
        cx.notify();
    }
    pub(crate) fn start(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.generation += 1;
        self.query = query.clone();
        self.hits.clear();
        self.message.clear();
        self.running = true;
        let generation = self.generation;
        let admitted = self.worker.submit(
            cx,
            move |session| {
                session.start(root, query);
                Ok(session.take_receiver())
            },
            move |search, result, cx| match result {
                Ok(Some(receiver)) => {
                    let executor = cx.background_executor().clone();
                    cx.spawn(async move |this, cx| {
                        let result = executor.spawn(async move { receiver.recv() }).await;
                        let _ = this.update(cx, |search, cx| {
                            if generation != search.generation {
                                return;
                            }
                            match result {
                                Ok(Ok(hits)) => {
                                    search.hits = hits;
                                    search.message.clear();
                                }
                                Ok(Err(error)) => search.message = error.to_string(),
                                Err(_) => {
                                    search.message = "検索workerが終了しました".to_owned();
                                }
                            }
                            search.running = false;
                            cx.notify();
                        });
                    })
                    .detach();
                }
                Ok(None) => {
                    search.running = false;
                    cx.notify();
                }
                Err(error) => {
                    search.message = error;
                    search.running = false;
                    cx.notify();
                }
            },
        );
        if let Err(error) = admitted {
            self.message = error;
            self.running = false;
            cx.notify();
        }
    }
}
