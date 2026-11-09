use crate::{
    extension_ui::{FeatureRegistry, ViewId, ZonePosition},
    kit::layout_profile::LayoutProfile,
    shell::{DockLayout, PanelTab, ShellState},
};
use desktop_client::{ConversationViewState, DocumentTab, TabRef};

pub(crate) fn capture_view_state(shell: &ShellState) -> ConversationViewState {
    ConversationViewState {
        // Compact では幅に依存した Panel 配置を保存しない。
        layout: (shell.layout_profile.get() == LayoutProfile::Wide).then(|| shell.layout.capture()),
        show_panel_controls: shell.show_panel_controls,
        workbench: Default::default(),
        workspace: None,
    }
}

pub(crate) fn apply_view_state(
    view: ConversationViewState,
    shell: &mut ShellState,
    documents: &[DocumentTab],
    registry: &FeatureRegistry,
) {
    let restored = view.layout.as_ref().and_then(|layout| {
        DockLayout::restore(layout, |reference| {
            let tab = deserialize_panel_tab(reference)?;
            match &tab {
                PanelTab::Document(id) => documents.iter().any(|document| &document.id == id),
                // TerminalはHost側の存在確認を再アタッチ時に任せ、参照IDだけで復元する。
                PanelTab::Tool { view, .. } if tab.terminal_id().is_some() => {
                    view.as_str() == crate::features::id::VIEW_TERMINAL
                }
                PanelTab::Tool { view, .. } => [
                    ZonePosition::Main,
                    ZonePosition::Left,
                    ZonePosition::Bottom,
                    ZonePosition::Right,
                ]
                .into_iter()
                .flat_map(|position| registry.zone_contributions(position))
                .any(|contribution| contribution.view.as_ref() == Some(view)),
            }
            .then_some(tab)
        })
    });
    shell.layout = restored.unwrap_or_default();
    shell.show_panel_controls = view.show_panel_controls;
    shell.synchronize_documents(documents);
}

fn deserialize_panel_tab(value: &TabRef) -> Option<PanelTab> {
    match value {
        TabRef::Document { id } => Some(PanelTab::Document(id.clone())),
        TabRef::Terminal { id } => Some(PanelTab::terminal(id.clone())),
        TabRef::View { id } => {
            if id.is_empty() {
                return None;
            }
            // ViewInstanceIdは "shared:<view>" か "<view>:<token>" の形を取る。
            let view = id
                .strip_prefix("shared:")
                .or_else(|| id.split_once(':').map(|(view, _)| view))
                .unwrap_or(id.as_str());
            if view.is_empty() {
                return None;
            }
            Some(PanelTab::tool_instance(ViewId::new(view), id.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_client::ZoneLayoutViewState;

    #[test]
    fn broken_layout_keeps_dirty_documents_in_the_default_zone() {
        let mut shell = ShellState::default();
        let document = DocumentTab {
            id: editor_core::DocumentId::new("unsaved"),
            display_name: "Untitled".into(),
            path: None,
            dirty: true,
            external_change: document::ExternalChange::Unchanged,
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

    #[test]
    fn compact_layout_is_not_persisted() {
        let shell = ShellState::default();

        shell.layout_profile.set(LayoutProfile::Compact);
        assert!(capture_view_state(&shell).layout.is_none());

        shell.layout_profile.set(LayoutProfile::Wide);
        assert!(capture_view_state(&shell).layout.is_some());
    }
}
