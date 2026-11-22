use crate::{
    DockLayout, PanelTab, ShellState, ViewInstanceId, extension_ui::FeatureRegistry,
    kit::layout_profile::LayoutProfile,
};
use desktop_client::{ConversationViewState, DocumentTab, TabRef};
use extension_ui_contract::{ViewId, ZonePosition};

/// Host Terminal を参照する旧 Terminal タブの個体接頭辞。
///
/// 旧保存値は Resource の種類を View 名で持たないため、読み込み時だけこの
/// 接頭辞で個体を作る。新形式は View の宣言から個体名を決める。
const TERMINAL_INSTANCE_PREFIX: &str = "terminal:";

pub fn capture_view_state(shell: &ShellState) -> ConversationViewState {
    ConversationViewState {
        // Compact では幅に依存した Panel 配置を保存しない。
        layout: (shell.layout_profile.get() == LayoutProfile::Wide).then(|| shell.layout.capture()),
        show_panel_controls: shell.show_panel_controls,
        workbench: Default::default(),
        workspace: None,
    }
}

pub fn apply_view_state(
    mut view: ConversationViewState,
    shell: &mut ShellState,
    documents: &[DocumentTab],
    registry: &FeatureRegistry,
) {
    // 共有状態に含まれないアクティブタブは、このClientの現在の選択を残す。
    let local_layout = shell.layout.capture();
    if let Some(layout) = view.layout.as_mut() {
        for panel in layout.zones.iter_mut().flat_map(|zone| &mut zone.panels) {
            for tab in panel
                .tabs
                .iter_mut()
                .chain(&mut panel.pinned_tabs)
                .chain(panel.active_tab.iter_mut())
            {
                match tab {
                    TabRef::View {
                        view: Some(view), ..
                    }
                    | TabRef::Resource {
                        view: Some(view), ..
                    } => {
                        *view = registry
                            .resolve_view(&ViewId::new(view.as_str()))
                            .as_str()
                            .to_owned();
                    }
                    _ => {}
                }
            }
        }
        layout.retain_active_tabs_from(&local_layout);
    }
    let restored = view.layout.as_ref().and_then(|layout| {
        DockLayout::restore(layout, |reference| {
            let tab = deserialize_panel_tab(reference, registry)?;
            match &tab {
                PanelTab::Document(id) => documents.iter().any(|document| &document.id == id),
                // 外部 Resource を参照するタブは、Resource の実在確認を再アタッチ時に任せ、
                // 参照 ID だけで復元する。どの View が参照するかは宣言から決まる。
                PanelTab::Tool { resource, .. } if resource.is_some() => true,
                PanelTab::Tool { view, .. } => {
                    [
                        ZonePosition::Main,
                        ZonePosition::Left,
                        ZonePosition::Bottom,
                        ZonePosition::Right,
                    ]
                    .into_iter()
                    .flat_map(|position| registry.zone_contributions(position))
                    .any(|contribution| contribution.view.as_ref() == Some(view))
                        || matches!(reference, TabRef::View { view: Some(_), .. })
                }
            }
            .then_some(tab)
        })
    });
    shell.layout = restored.unwrap_or_default();
    shell.unregistered_view_types = shell
        .layout
        .placements()
        .into_iter()
        .filter_map(|(tab, _)| tab.view_id().cloned())
        .filter(|view| registry.declaration(view).is_none())
        .collect();
    shell.show_panel_controls = view.show_panel_controls;
    shell.synchronize_documents(documents);
}

fn deserialize_panel_tab(value: &TabRef, registry: &FeatureRegistry) -> Option<PanelTab> {
    match value {
        TabRef::Document { id } => Some(PanelTab::Document(id.clone())),
        TabRef::Resource { view, id } => {
            // View 未指定の旧形式は Terminal として保存されていた。
            let view = view
                .as_deref()
                .map(|view| registry.resolve_view(&ViewId::new(view)))
                .or_else(|| {
                    let terminal = registry.resolve_view(&ViewId::new(crate::ids::VIEW_TERMINAL));
                    (value.legacy_terminal_id().is_some()
                        && registry.declaration(&terminal).is_some_and(|declaration| {
                            declaration.tab_reference
                                == extension_ui_contract::TabReference::ExternalResource
                        }))
                    .then_some(terminal)
                })?;
            let instance = if view == registry.resolve_view(&ViewId::new(crate::ids::VIEW_TERMINAL))
            {
                ViewInstanceId::new(format!("{TERMINAL_INSTANCE_PREFIX}{id}"))
            } else {
                ViewInstanceId::new(format!(
                    "resource:{}:{}:{id}",
                    view.as_str().len(),
                    view.as_str()
                ))
            };
            Some(PanelTab::external(view, instance, Some(id.clone())))
        }
        TabRef::View { id, view } => {
            if id.is_empty() {
                return None;
            }
            // ViewInstanceIdは "shared:<view>" か "<view>:<token>" の形を取る。
            let view = view.as_deref().unwrap_or_else(|| {
                id.strip_prefix("shared:")
                    .or_else(|| id.split_once(':').map(|(view, _)| view))
                    .unwrap_or(id.as_str())
            });
            if view.is_empty() {
                return None;
            }
            Some(PanelTab::tool_instance(
                registry.resolve_view(&ViewId::new(view)),
                id.clone(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PanelId;
    use desktop_client::ZoneLayoutViewState;

    #[test]
    fn arbitrary_instance_id_restores_with_its_declared_view() {
        use extension_ui_contract::{FeatureDescriptor, UiContribution, UiSlot};
        let mut registry = crate::test_support::registry();
        registry
            .register(
                FeatureDescriptor::core("example").contributes(
                    UiContribution::view(
                        "example.view",
                        UiSlot::BottomDock,
                        "Example",
                        "example",
                        0,
                    )
                    .multiple_instances(),
                ),
            )
            .unwrap();
        let tab = PanelTab::tool_instance("example.view", "opaque-instance");
        let mut original = ShellState::default();
        original.layout_profile.set(LayoutProfile::Wide);
        original
            .layout
            .open_tab(ZonePosition::Bottom, tab.clone())
            .unwrap();
        original.layout.set_tab_pinned(&tab, true);
        let saved = serde_json::to_string(&capture_view_state(&original)).unwrap();
        let decoded = serde_json::from_str(&saved).unwrap();
        let mut restored = ShellState::default();
        restored.layout_profile.set(LayoutProfile::Wide);
        apply_view_state(decoded, &mut restored, &[], &registry);
        assert!(restored.layout.find_tab(&tab).is_some());
        assert!(restored.layout.tab_is_pinned(&tab));
        let legacy: TabRef =
            serde_json::from_str(r#"{"kind":"view","id":"example.view:old"}"#).unwrap();
        assert_eq!(
            deserialize_panel_tab(&legacy, &registry)
                .unwrap()
                .view_id()
                .unwrap()
                .as_str(),
            "example.view"
        );
    }

    #[test]
    fn pending_extension_tabs_survive_until_the_view_is_registered() {
        use extension_ui_contract::{FeatureDescriptor, UiContribution, UiSlot};
        let mut registry = crate::test_support::registry();
        let pending = PanelTab::tool_instance("example.later.view", "opaque-id");
        let resource = PanelTab::external("example.later.view", "other-id", Some("session".into()));
        let mut original = ShellState::default();
        original.layout_profile.set(LayoutProfile::Wide);
        original
            .layout
            .open_tab(ZonePosition::Bottom, pending.clone())
            .unwrap();
        original
            .layout
            .open_tab(ZonePosition::Bottom, resource.clone())
            .unwrap();
        let saved = capture_view_state(&original);
        let mut restored = ShellState::default();
        restored.layout_profile.set(LayoutProfile::Wide);
        apply_view_state(saved, &mut restored, &[], &registry);
        assert!(restored.layout.find_tab(&pending).is_some());
        let restored_resource = restored
            .layout
            .placements()
            .into_iter()
            .map(|(tab, _)| tab)
            .find(|tab| tab.resource() == Some("session"))
            .unwrap();
        assert!(!restored.view_visible(&pending));
        assert!(!restored.view_visible(&restored_resource));
        let persisted = capture_view_state(&restored);
        registry
            .register(
                FeatureDescriptor::core("example.later").contributes(
                    UiContribution::view(
                        "example.later.view",
                        UiSlot::BottomDock,
                        "Example",
                        "example",
                        0,
                    )
                    .multiple_instances(),
                ),
            )
            .unwrap();
        apply_view_state(persisted, &mut restored, &[], &registry);
        assert!(restored.layout.find_tab(&pending).is_some());
        assert!(restored.view_visible(&pending));
        assert!(restored.view_visible(&restored_resource));
    }

    #[test]
    fn old_terminal_view_reference_restores_to_declared_id_with_pin() {
        let mut registry = crate::test_support::registry();
        let old = ViewId::new(crate::ids::VIEW_TERMINAL);
        let new = ViewId::new("lapis.terminal.view");
        let tab = PanelTab::external(old.clone(), "terminal:before", Some("session".into()));
        let mut original = ShellState::default();
        original.layout_profile.set(LayoutProfile::Wide);
        original
            .layout
            .open_tab(ZonePosition::Bottom, tab.clone())
            .unwrap();
        original.layout.set_tab_pinned(&tab, true);
        let saved = capture_view_state(&original);
        assert!(registry.rekey_view(&old, &new));
        let mut restored = ShellState::default();
        restored.layout_profile.set(LayoutProfile::Wide);
        apply_view_state(saved, &mut restored, &[], &registry);
        let tab = restored
            .layout
            .placements()
            .into_iter()
            .map(|(tab, _)| tab)
            .find(|tab| tab.resource() == Some("session"))
            .unwrap();
        assert_eq!(tab.view_id(), Some(&new));
        assert!(restored.layout.tab_is_pinned(&tab));
    }

    #[test]
    fn external_resources_restore_per_view_and_legacy_terminal_keeps_its_owner() {
        use extension_ui_contract::{FeatureDescriptor, UiContribution, UiSlot};

        let mut registry = crate::test_support::registry();
        registry
            .register(
                FeatureDescriptor::core("aaa.example").contributes(
                    UiContribution::view(
                        "aaa.example.view",
                        UiSlot::BottomDock,
                        "Example",
                        "example",
                        0,
                    )
                    .references_external_resource(),
                ),
            )
            .unwrap();
        let resource = |view: Option<&str>| TabRef::Resource {
            view: view.map(str::to_owned),
            id: "same-id".to_owned(),
        };
        let legacy = deserialize_panel_tab(&resource(None), &registry).unwrap();
        let terminal =
            deserialize_panel_tab(&resource(Some(crate::ids::VIEW_TERMINAL)), &registry).unwrap();
        let other = deserialize_panel_tab(&resource(Some("aaa.example.view")), &registry).unwrap();
        assert_eq!(legacy, terminal);
        assert_eq!(
            legacy.view_id().unwrap().as_str(),
            crate::ids::VIEW_TERMINAL
        );
        assert_ne!(other.instance_id(), terminal.instance_id());
        assert_eq!(other.view_id().unwrap().as_str(), "aaa.example.view");
    }

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
            &crate::test_support::registry(),
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
        let registry = crate::test_support::registry();
        let tab_a = PanelTab::Document(document_a.id.clone());
        let tab_b = PanelTab::Document(document_b.id.clone());
        let search = PanelTab::tool(crate::ids::VIEW_SEARCH);
        let files = PanelTab::tool(crate::ids::VIEW_FILES);

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
        let registry = crate::test_support::registry();
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
