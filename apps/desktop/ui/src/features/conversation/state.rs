use crate::{components::action_button, theme};
use gpui::{Context, Render, Window, div, prelude::*};
use lapis_desktop_client::{ConversationRecord, Project, SourceRole};
use lapis_editor_core::{ConversationId, ProjectId};
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) enum ConversationEvent {
    RegisterKnownSource(ProjectId, PathBuf),
    RegisterProject,
    RegisterSource(ProjectId),
    Create {
        project: ProjectId,
        primary: PathBuf,
        sub: Option<PathBuf>,
    },
    Switch(ConversationId),
    Source(SourceRole),
}

pub(crate) struct ConversationFeature {
    available_sources: Vec<(PathBuf, String)>,
    pub(crate) records: Vec<ConversationRecord>,
    pub(crate) active: Option<ConversationId>,
    projects: Vec<Project>,
    selected_project: Option<ProjectId>,
    primary: Option<PathBuf>,
    sub: Option<PathBuf>,
    error: Option<String>,
}
impl gpui::EventEmitter<ConversationEvent> for ConversationFeature {}
impl ConversationFeature {
    pub(crate) fn new() -> Self {
        Self {
            available_sources: Vec::new(),
            records: Vec::new(),
            active: None,
            projects: Vec::new(),
            selected_project: None,
            primary: None,
            sub: None,
            error: None,
        }
    }
    fn select_project(&mut self, id: ProjectId) {
        self.primary = self
            .projects
            .iter()
            .find(|project| project.id() == &id)
            .map(|project| project.root().to_owned());
        self.sub = None;
        self.selected_project = Some(id);
    }
    pub(crate) fn update(
        &mut self,
        available_sources: Vec<(PathBuf, String)>,
        records: Vec<ConversationRecord>,
        projects: Vec<Project>,
        error: Option<String>,
        active: ConversationId,
        cx: &mut Context<Self>,
    ) {
        self.available_sources = available_sources;
        let switched = self.active.as_ref() != Some(&active);
        self.records = records;
        self.projects = projects;
        self.error = error;
        if switched || self.selected_project.is_none() {
            let selected = self
                .records
                .iter()
                .find(|record| record.id == active)
                .and_then(|record| record.environment.as_ref())
                .map(|environment| environment.project().clone())
                .or_else(|| self.projects.first().map(|project| project.id().clone()));
            if let Some(selected) = selected {
                self.select_project(selected);
            }
        }
        self.active = Some(active);
        cx.notify();
    }
}
impl Render for ConversationFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sources = self
            .projects
            .iter()
            .find(|project| Some(project.id()) == self.selected_project.as_ref())
            .map(|project| project.sources().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut view =
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .child(action_button("Projectを登録…", false).on_click(
                    cx.listener(|_, _, _, cx| cx.emit(ConversationEvent::RegisterProject)),
                ))
                .children(self.error.clone())
                .children(self.projects.iter().enumerate().map(|(index, project)| {
                    let id = project.id().clone();
                    div().id(("project-selector", index)).child(
                        action_button(
                            project.display_name().to_owned(),
                            Some(&id) == self.selected_project.as_ref(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_project(id.clone());
                            cx.notify();
                        })),
                    )
                }));
        if let Some(project) = self.selected_project.clone() {
            for (index, (root, name)) in self.available_sources.iter().enumerate() {
                if sources.contains(root) {
                    continue;
                }
                let root = root.clone();
                let project = project.clone();
                view = view.child(div().id(("host-source", index)).child(
                    action_button(format!("Host環境を追加: {name}"), false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            cx.emit(ConversationEvent::RegisterKnownSource(
                                project.clone(),
                                root.clone(),
                            ))
                        },
                    )),
                ));
            }
            view = view.child(
                action_button("既存Repository / フォルダを追加…", false).on_click(cx.listener(
                    move |_, _, _, cx| cx.emit(ConversationEvent::RegisterSource(project.clone())),
                )),
            );
        }
        view = view
            .child("新しいTaskの環境を選択")
            .children(sources.iter().enumerate().map(|(index, source)| {
                let primary = source.clone();
                let sub = source.clone();
                div()
                    .id(("task-source", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(source.display().to_string())
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                action_button("Primary", self.primary.as_ref() == Some(source))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.sub.as_ref() == Some(&primary) {
                                            this.sub = None;
                                        }
                                        this.primary = Some(primary.clone());
                                        cx.notify();
                                    })),
                            )
                            .when(self.primary.as_ref() != Some(source), |view| {
                                view.child(
                                    action_button("Sub（任意）", self.sub.as_ref() == Some(source))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.sub = if this.sub.as_ref() == Some(&sub) {
                                                None
                                            } else {
                                                Some(sub.clone())
                                            };
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
            }));
        if let (Some(project), Some(primary)) =
            (self.selected_project.clone(), self.primary.clone())
        {
            let sub = self.sub.clone();
            view = view.child(action_button("Taskを作成", true).on_click(cx.listener(
                move |_, _, _, cx| {
                    cx.emit(ConversationEvent::Create {
                        project: project.clone(),
                        primary: primary.clone(),
                        sub: sub.clone(),
                    });
                },
            )));
        }
        view = view.children(
            self.records
                .iter()
                .filter(|record| {
                    record.environment.as_ref().is_some_and(|environment| {
                        Some(environment.project()) == self.selected_project.as_ref()
                    })
                })
                .enumerate()
                .map(|(index, record)| {
                    let id = record.id.clone();
                    let active = self.active.as_ref() == Some(&id);
                    div()
                        .id(("project-task", index))
                        .p_2()
                        .bg(if active {
                            theme::colors().button_background_selected
                        } else {
                            theme::colors().background_tertiary
                        })
                        .child(record.title.clone())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ConversationEvent::Switch(id.clone()))
                        }))
                }),
        );
        view = view.children(
            self.records
                .iter()
                .filter(|record| {
                    record.environment.is_none() && !record.workspace.open_documents.is_empty()
                })
                .enumerate()
                .map(|(index, record)| {
                    let id = record.id.clone();
                    div()
                        .id(("scratch-draft", index))
                        .p_2()
                        .child("未所属の下書き（Task外）")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ConversationEvent::Switch(id.clone()))
                        }))
                }),
        );
        if let Some(record) = self
            .records
            .iter()
            .find(|record| Some(&record.id) == self.active.as_ref())
            && let Some(environment) = &record.environment
        {
            view = view.child("操作対象（Editor / Terminal / Git）").child(
                action_button(
                    format!(
                        "Primary: {}",
                        environment.root(SourceRole::Primary).unwrap().display()
                    ),
                    record.source_role == SourceRole::Primary,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(ConversationEvent::Source(SourceRole::Primary))
                })),
            );
            if let Some(sub) = environment.root(SourceRole::Sub) {
                view = view.child(
                    action_button(
                        format!("Sub: {}", sub.display()),
                        record.source_role == SourceRole::Sub,
                    )
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(ConversationEvent::Source(SourceRole::Sub))
                    })),
                );
            }
        }
        view
    }
}
