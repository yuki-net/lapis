use crate::{
    extension_ui::{FeatureRegistry, PanelPosition, ViewId},
    shell::{DockLayout, PanelTab, ShellState},
};
use lapis_desktop_client::{ConversationViewState, DocumentTab};

pub(crate) fn capture_view_state(shell: &ShellState) -> ConversationViewState {
    ConversationViewState {
        layout: Some(shell.layout.capture()),
        show_panel_controls: shell.show_panel_controls,
        workbench: Default::default(),
    }
}

pub(crate) fn apply_view_state(
    view: ConversationViewState,
    shell: &mut ShellState,
    documents: &[DocumentTab],
    registry: &FeatureRegistry,
) {
    let restored = view.layout.as_ref().and_then(|layout| {
        DockLayout::restore(layout, |raw| {
            let tab = deserialize_panel_tab(raw)?;
            match &tab {
                PanelTab::Document(id) => documents.iter().any(|document| &document.id == id),
                PanelTab::Tool { view, .. } => [
                    PanelPosition::Main,
                    PanelPosition::Left,
                    PanelPosition::Bottom,
                    PanelPosition::Right,
                ]
                .into_iter()
                .flat_map(|position| registry.panel_contributions(position))
                .any(|contribution| contribution.view.as_ref() == Some(view)),
            }
            .then_some(tab)
        })
    });
    shell.layout = restored.unwrap_or_default();
    shell.show_panel_controls = view.show_panel_controls;
    shell.synchronize_documents(documents);
}

fn deserialize_panel_tab(value: &str) -> Option<PanelTab> {
    if let Some(id) = value.strip_prefix("document:") {
        return (!id.is_empty())
            .then(|| PanelTab::Document(lapis_editor_core::DocumentId::new(id)));
    }
    let (view, instance) = value.strip_prefix("tool:")?.split_once(':')?;
    if view.is_empty() || instance.is_empty() {
        return None;
    }
    Some(PanelTab::tool_instance(
        ViewId::new(view),
        instance.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_desktop_client::ZoneLayoutViewState;

    #[test]
    fn broken_layout_keeps_dirty_documents_in_the_default_zone() {
        let mut shell = ShellState::default();
        let document = DocumentTab {
            id: lapis_editor_core::DocumentId::new("unsaved"),
            display_name: "Untitled".into(),
            path: None,
            dirty: true,
            external_change: lapis_document::ExternalChange::Unchanged,
            encoding: Default::default(),
            active: true,
        };
        let view = ConversationViewState {
            layout: Some(ZoneLayoutViewState {
                version: 999,
                zones: Vec::new(),
                bottom_span_left: false,
                bottom_span_right: false,
            }),
            ..Default::default()
        };
        apply_view_state(
            view,
            &mut shell,
            std::slice::from_ref(&document),
            &crate::features::bundled_registry(),
        );
        let tab = PanelTab::Document(document.id.clone());
        assert_eq!(
            shell
                .layout
                .placements()
                .iter()
                .filter(|(placed, _)| placed == &tab)
                .count(),
            1
        );
        assert!(document.dirty);
        assert!(shell.layout.valid());
    }
}
