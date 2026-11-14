use crate::{
    extension_ui::FeatureRegistry,
    kit::layout_profile::LayoutProfile,
    shell::{DockLayout, PanelTab, ShellState},
};
use desktop_client::{ConversationViewState, DocumentTab, TabRef};
use extension_ui_contract::{ViewId, ZonePosition};

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
    mut view: ConversationViewState,
    shell: &mut ShellState,
    documents: &[DocumentTab],
    registry: &FeatureRegistry,
) {
    // 共有状態に含まれないアクティブタブは、このClientの現在の選択を残す。
    let local_layout = shell.layout.capture();
    if let Some(layout) = view.layout.as_mut() {
        layout.retain_active_tabs_from(&local_layout);
    }
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
    use crate::shell::PanelId;
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

    fn document(id: &str) -> DocumentTab {
        DocumentTab {
            id: editor_core::DocumentId::new(id),
            display_name: id.into(),
            path: None,
            dirty: false,
            external_change: document::ExternalChange::Unchanged,
            encoding: Default::default(),
            active: false,
        }
    }

    fn main_panel(shell: &ShellState) -> PanelId {
        shell
            .layout
            .first_panel_in_zone(ZonePosition::Main)
            .expect("Mainpanelを保持する")
    }

    fn tab_order(shell: &ShellState, panel: &PanelId) -> Vec<PanelTab> {
        shell.layout.panel(panel).unwrap().tabs.clone()
    }

    #[test]
    fn desktop_and_web_restore_the_same_tab_composition() {
        let document_a = document("doc-a");
        let document_b = document("doc-b");
        let documents = vec![document_a.clone(), document_b.clone()];
        let registry = crate::features::bundled_registry();
        let tab_a = PanelTab::Document(document_a.id.clone());
        let tab_b = PanelTab::Document(document_b.id.clone());
        let search = PanelTab::tool(crate::features::id::VIEW_SEARCH);
        let files = PanelTab::tool(crate::features::id::VIEW_FILES);

        let mut desktop = ShellState::default();
        desktop.layout_profile.set(LayoutProfile::Wide);
        desktop
            .layout
            .open_tab(ZonePosition::Main, tab_b.clone())
            .unwrap();
        desktop
            .layout
            .open_tab(ZonePosition::Main, tab_a.clone())
            .unwrap();
        assert!(desktop.layout.set_tab_pinned(&tab_a, true));
        desktop
            .layout
            .open_tab(ZonePosition::Right, search.clone())
            .unwrap();
        let files_panel = desktop.layout.find_tab(&files).unwrap();
        assert!(desktop.layout.set_panel_open(&files_panel, false));

        // Desktopの配置をJSON経由で渡し、Webが同じタブ構成を復元する。
        let shared: ConversationViewState = serde_json::from_str(
            &serde_json::to_string(&capture_view_state(&desktop).for_shared_workbench()).unwrap(),
        )
        .unwrap();
        let mut web = ShellState::default();
        web.layout_profile.set(LayoutProfile::Wide);
        apply_view_state(shared, &mut web, &documents, &registry);

        assert_eq!(
            capture_view_state(&web).for_shared_workbench(),
            capture_view_state(&desktop).for_shared_workbench()
        );
        assert_eq!(
            tab_order(&web, &main_panel(&web)),
            vec![tab_a.clone(), tab_b.clone()]
        );
        assert!(web.layout.tab_is_pinned(&tab_a));
        assert!(
            !web.layout
                .panel(&web.layout.find_tab(&files).unwrap())
                .unwrap()
                .open
        );

        // Webでタブを動かした構成もDesktopへ同じ形で反映する。
        web.layout
            .move_tab_to_zone(
                ZonePosition::Right,
                tab_b.clone(),
                crate::time::Instant::now(),
            )
            .unwrap();
        apply_view_state(
            capture_view_state(&web).for_shared_workbench(),
            &mut desktop,
            &documents,
            &registry,
        );

        assert_eq!(
            capture_view_state(&desktop).for_shared_workbench(),
            capture_view_state(&web).for_shared_workbench()
        );
        assert_eq!(tab_order(&desktop, &main_panel(&desktop)), vec![tab_a]);
        let right = desktop.layout.find_tab(&search).unwrap();
        assert_eq!(tab_order(&desktop, &right), vec![search, tab_b]);
    }

    #[test]
    fn active_tab_stays_independent_per_client() {
        let document_a = document("doc-a");
        let document_b = document("doc-b");
        let documents = vec![document_a.clone(), document_b.clone()];
        let registry = crate::features::bundled_registry();
        let tab_a = PanelTab::Document(document_a.id.clone());
        let tab_b = PanelTab::Document(document_b.id.clone());

        let mut desktop = ShellState::default();
        desktop.layout_profile.set(LayoutProfile::Wide);
        desktop
            .layout
            .open_tab(ZonePosition::Main, tab_b.clone())
            .unwrap();
        desktop
            .layout
            .open_tab(ZonePosition::Main, tab_a.clone())
            .unwrap();
        let desktop_main = main_panel(&desktop);
        assert_eq!(
            desktop.layout.panel(&desktop_main).unwrap().active,
            Some(tab_a.clone())
        );

        // 共有状態にはアクティブタブを含めないため、Webは先頭タブを選ぶ。
        let mut web = ShellState::default();
        web.layout_profile.set(LayoutProfile::Wide);
        apply_view_state(
            capture_view_state(&desktop).for_shared_workbench(),
            &mut web,
            &documents,
            &registry,
        );
        assert_eq!(
            web.layout.panel(&main_panel(&web)).unwrap().active,
            Some(tab_b.clone())
        );
        assert_eq!(
            desktop.layout.panel(&desktop_main).unwrap().active,
            Some(tab_a.clone())
        );

        // Webの選択を同期してもDesktopの選択は変わらない。
        apply_view_state(
            capture_view_state(&web).for_shared_workbench(),
            &mut desktop,
            &documents,
            &registry,
        );
        assert_eq!(
            desktop.layout.panel(&main_panel(&desktop)).unwrap().active,
            Some(tab_a)
        );
        assert_eq!(
            web.layout.panel(&main_panel(&web)).unwrap().active,
            Some(tab_b.clone())
        );

        // Desktopの選択を同期してもWebの選択は変わらない。
        apply_view_state(
            capture_view_state(&desktop).for_shared_workbench(),
            &mut web,
            &documents,
            &registry,
        );
        assert_eq!(
            web.layout.panel(&main_panel(&web)).unwrap().active,
            Some(tab_b)
        );
    }
}
