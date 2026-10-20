use crate::{client_adapter::ClientWorker, commands::UiCommand};
use gpui::{Context, Timer};
use lapis_desktop_client::WorkspaceSearchSession;
use lapis_workspace::SearchHit;
use std::{path::PathBuf, time::Duration};

pub(crate) struct SearchFeature {
    worker: ClientWorker<WorkspaceSearchSession>,
    pub(super) root: Option<PathBuf>,
    pub(super) query: String,
    pub(super) hits: Vec<SearchHit>,
    pub(super) message: String,
    pub(super) running: bool,
    generation: u64,
    busy: bool,
}
impl gpui::EventEmitter<UiCommand> for SearchFeature {}
impl SearchFeature {
    pub(crate) fn new(session: WorkspaceSearchSession, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                if this.update(cx, |search, cx| search.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            worker: ClientWorker::new(session),
            root: None,
            query: String::new(),
            hits: Vec::new(),
            message: String::new(),
            running: false,
            generation: 0,
            busy: false,
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
        if let Err(error) = self.worker.try_request(move |session| {
            session.start(root, query);
            Ok(())
        }) {
            self.message = error;
            self.running = false;
        }
        cx.notify();
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.running {
            return;
        }
        self.busy = true;
        let generation = self.generation;
        let admitted = self.worker.submit(
            cx,
            |session| {
                session.refresh();
                Ok((
                    session.hits().to_vec(),
                    session.is_running(),
                    session.error().unwrap_or_default().to_owned(),
                ))
            },
            move |search, result, cx| {
                search.busy = false;
                if generation != search.generation {
                    return;
                }
                match result {
                    Ok((hits, running, message)) => {
                        search.hits = hits;
                        search.running = running;
                        search.message = message;
                    }
                    Err(error) => {
                        search.message = error;
                        search.running = false;
                    }
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.busy = false;
            self.message = error;
            cx.notify();
        }
    }
}
