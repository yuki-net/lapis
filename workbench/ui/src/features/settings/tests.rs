use super::*;
use crate::features::settings_coordinator::SettingsCoordinator;
use crate::theme;
use desktop_client::{HostDevice, HostWebStatus, WebAccessControl};
use extension_ui_contract::ThemeId;
use gpui::px;
use settings::{
    SettingValue, SettingsDocument, SettingsError, SettingsRepository, SettingsScope, SyntaxTheme,
};
use std::path::PathBuf;
use std::sync::Arc;

struct MemorySettings;

#[gpui::test]
fn extension_locale_is_selectable_and_falls_back_when_disconnected(cx: &mut gpui::TestAppContext) {
    let (feature, session) = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            let session = settings.extension_contributions();
            let generation = session.begin_reload();
            session.apply(generation, serde_json::from_value(serde_json::json!({"extensions":[], "contributions":[{
                "extension_id":"dev.example.french", "declaration":{"schema":1,"translations":[{"target":"lapis.core","locale":"fr-FR","messages":{"settings.search.placeholder":"Rechercher"}}]}
            }]})).unwrap()).unwrap();
            SettingsCoordinator::init(settings, cx);
            let feature = cx.new(SettingsFeature::new);
            feature.update(cx, |feature, cx| feature.set_extension_contributions(Some(session.clone()), cx));
            (feature, session)
        });
    cx.update(|cx| {
        feature.update(cx, |feature, cx| {
            let definition = settings::catalog()
                .settings_of("language.locale")
                .find(|definition| definition.key == settings::UI_LOCALE)
                .unwrap();
            assert!(
                feature
                    .options(definition, cx)
                    .iter()
                    .any(|(value, _)| value.as_text() == Some("fr-FR"))
            );
            feature.choose(settings::UI_LOCALE, SettingValue::text("fr-FR"), cx);
        });
    });
    wait_for_settings(cx);
    cx.update(|cx| {
        assert_eq!(
            feature.read(cx).t("settings.search.placeholder"),
            "Rechercher"
        )
    });
    session.disconnect();
    cx.update(|cx| feature.update(cx, |feature, cx| feature.refresh_extension_locales(cx)));
    cx.update(|cx| {
        assert_eq!(
            feature.read(cx).t("settings.search.placeholder"),
            "Search settings"
        )
    });
}

#[gpui::test]
fn failed_management_keeps_current_and_next_state(cx: &mut gpui::TestAppContext) {
    let (feature, session) = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            let session = settings.extension_contributions();
            let generation = session.begin_reload();
            session.apply(generation, serde_json::from_value(serde_json::json!({"extensions":[{
                "id":"dev.example.terminal","version":"1.0.0","installed":true,"enabled":true,"active":true
            }], "contributions":[]})).unwrap()).unwrap();
            SettingsCoordinator::init(settings, cx);
            let feature = cx.new(SettingsFeature::new);
            feature.update(cx, |feature, cx| {
                feature.set_extension_contributions(Some(session.clone()), cx);
                feature.set_extension_manager(Some(Arc::new(|_, _, _| Err("forbidden".into()))));
                feature.set_workspace(None, Some("workspace-1".into()), cx);
                assert!(feature.visible_pages().contains(&"extensions.settings"));
                feature.set_extension_enabled("dev.example.terminal".into(), false, cx);
            });
            (feature, session)
        });
    let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(3);
    while cx.update(|cx| feature.read(cx).extension_management_busy) {
        cx.run_until_parked();
        assert!(crate::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    cx.update(|cx| assert_eq!(feature.read(cx).message, "forbidden"));
    let snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.extensions()[0].active, Some(true));
    assert!(snapshot.extensions()[0].enabled);
    session.disconnect();
    cx.update(|cx| {
        assert!(
            !feature
                .read(cx)
                .visible_pages()
                .contains(&"extensions.settings")
        )
    });
}

#[cfg(not(target_family = "wasm"))]
#[gpui::test]
fn capability_failure_and_workspace_change_do_not_show_stale_approval(
    cx: &mut gpui::TestAppContext,
) {
    let (feature, session) = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        let session = settings.extension_contributions();
        let generation = session.begin_reload();
        session
            .apply(
                generation,
                serde_json::from_value(serde_json::json!({"extensions":[{
                "id":"dev.example.pty", "version":"1.0.0", "installed":true, "enabled":true
            }], "contributions":[]}))
                .unwrap(),
            )
            .unwrap();
        SettingsCoordinator::init(settings, cx);
        let feature = cx.new(SettingsFeature::new);
        feature.update(cx, |feature, cx| {
            feature.set_extension_contributions(Some(session.clone()), cx);
            feature.set_extension_capability_manager(Some(Arc::new(|_, id, action| {
                if action.is_some() {
                    return Err("forbidden".into());
                }
                Ok(desktop_client::HostExtensionCapabilitiesResponse {
                    id,
                    manifest_digest: "a".repeat(64),
                    capabilities: vec![desktop_client::HostExtensionCapability {
                        id: "process.pty".into(),
                        version: "1.0.0".into(),
                        approved: false,
                        supported: true,
                    }],
                })
            })));
            feature.set_workspace(None, Some("workspace-1".into()), cx);
            feature.manage_capabilities("dev.example.pty".into(), None, cx);
        });
        (feature, session)
    });
    let wait = |cx: &mut gpui::TestAppContext| {
        let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(3);
        while cx.update(|cx| feature.read(cx).capability_busy) {
            cx.run_until_parked();
            assert!(crate::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    };
    wait(cx);
    feature.update(cx, |feature, cx| {
        assert!(!feature.capability_info.as_ref().unwrap().capabilities[0].approved);
        feature.manage_capabilities(
            "dev.example.pty".into(),
            Some(desktop_client::HostExtensionSetCapabilityRequest {
                id: "dev.example.pty".into(),
                manifest_digest: "a".repeat(64),
                capability: "process.pty".into(),
                version: "1.0.0".into(),
                approved: true,
            }),
            cx,
        );
    });
    wait(cx);
    feature.update(cx, |feature, cx| {
        assert_eq!(feature.message, "forbidden");
        assert!(!feature.capability_info.as_ref().unwrap().capabilities[0].approved);
        feature.set_workspace(None, Some("workspace-2".into()), cx);
        assert!(feature.capability_info.is_none());
    });
    session.disconnect();
}

#[gpui::test]
fn contributed_settings_are_searchable_and_saved(cx: &mut gpui::TestAppContext) {
    let (feature, session) = cx.update(|cx| {
            theme::init(&ThemeId::new("lapis.dark"), None, cx);
            let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
            let session = settings.extension_contributions();
            let generation = session.begin_reload();
            session.apply(generation, serde_json::from_value(serde_json::json!({"extensions":[], "contributions":[{
                "extension_id":"dev.example.terminal",
                "declaration":{"schema":1,"settings":[{"definition":{"name":"size","value_kind":"integer","scopes":["user"],"default":12},"label":"terminal.size"}],"translations":[{"target":"dev.example.terminal","locale":"en-US","messages":{"terminal.size":"Terminal Size"}}]}
            }]})).unwrap()).unwrap();
            SettingsCoordinator::init(settings, cx);
            let feature = cx.new(SettingsFeature::new);
            feature.update(cx, |feature, cx| feature.set_extension_contributions(Some(session.clone()), cx));
            (feature, session)
        });
    cx.update(|cx| {
        feature.update(cx, |feature, cx| {
            feature.query = "terminal size".into();
            assert!(feature.visible_pages().contains(&"extensions.settings"));
            assert_eq!(feature.extension_rows()[0].1, "Terminal Size");
            feature.save_extension(
                "extensions/dev.example.terminal/size".into(),
                Some(SettingValue::integer(18)),
                cx,
            );
        });
    });
    let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        cx.run_until_parked();
        let value = session
            .resolved()
            .unwrap()
            .extension_setting("extensions/dev.example.terminal/size")
            .unwrap()
            .value()
            .cloned();
        if value == Some(SettingValue::integer(18)) {
            break;
        }
        assert!(
            crate::time::Instant::now() < deadline,
            "拡張設定が保存されません"
        );
        std::thread::yield_now();
    }
    session.disconnect();
    cx.update(|cx| {
        assert!(
            !feature
                .read(cx)
                .visible_pages()
                .contains(&"extensions.settings")
        )
    });
}

#[gpui::test]
fn extension_terminal_hides_legacy_settings_pages(cx: &mut gpui::TestAppContext) {
    let feature = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        cx.new(SettingsFeature::new)
    });
    cx.update(|cx| {
        feature.update(cx, |feature, _| {
            assert!(feature.visible_pages().contains(&"terminal.font"));
            feature.set_legacy_terminal_settings_hidden(true);
            assert!(!feature.visible_pages().contains(&"terminal.font"));
            assert!(!feature.visible_pages().contains(&"terminal.startup"));
        });
    });
}

#[gpui::test]
fn search_filters_settings_and_restores_them_when_cleared(cx: &mut gpui::TestAppContext) {
    let feature = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        cx.new(SettingsFeature::new)
    });
    cx.update(|cx| {
        let search = feature.read(cx).search.clone();
        search.update(cx, |search, cx| {
            search.set_text("terminal font size".into(), cx)
        });
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let feature = feature.read(cx);
        assert_eq!(feature.visible_pages(), vec!["terminal.font"]);
        let catalog = settings::catalog();
        let size = catalog
            .settings_of("terminal.font")
            .find(|field| field.key == settings::TERMINAL_FONT_SIZE)
            .unwrap();
        let family = catalog
            .settings_of("terminal.font")
            .find(|field| field.key == settings::TERMINAL_FONT_FAMILY)
            .unwrap();
        assert!(feature.field_matches_search(size));
        assert!(!feature.field_matches_search(family));
    });
    cx.update(|cx| {
        let search = feature.read(cx).search.clone();
        search.update(cx, |search, cx| {
            search.set_text("no-such-setting-xyz".into(), cx)
        });
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(feature.read(cx).visible_pages().is_empty()));
    cx.update(|cx| {
        let search = feature.read(cx).search.clone();
        search.update(cx, |search, cx| search.set_text(String::new(), cx));
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(feature.read(cx).visible_pages().len() > 1));
}

#[cfg(debug_assertions)]
#[gpui::test]
fn custom_choice_opens_and_saves_a_value(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
    });
    let (feature, visual) = cx.add_window_view(|_, cx| SettingsFeature::new(cx));
    let expected = visual.update(|_, cx| {
        feature.update(cx, |feature, cx| {
            let definition = settings::catalog()
                .settings_of("appearance.theme")
                .next()
                .unwrap();
            feature.options(definition, cx)[1].0.clone()
        })
    });
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    assert!(visual.debug_bounds("settings-search").is_some());
    let choice = visual
        .debug_bounds("settings-choice-ui.theme")
        .expect("theme choice");
    visual.simulate_click(choice.center(), gpui::Modifiers::none());
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    let option = visual.debug_bounds("ui.theme-1").expect("second theme");
    visual.simulate_click(option.center(), gpui::Modifiers::none());
    feature.read_with(visual, |feature, _| assert!(feature.open_choice.is_none()));
    wait_for_settings(cx);
    cx.update(|cx| {
        assert_eq!(
            feature.read(cx).values.value(settings::UI_THEME),
            Some(&expected)
        )
    });
}

#[cfg(debug_assertions)]
#[gpui::test]
fn navigation_scrolls_to_sections_without_moving_navigation(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
    });
    let (feature, visual) = cx.add_window_view(|_, cx| SettingsFeature::new(cx));
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    let nav = visual
        .debug_bounds("main-settings-nav")
        .expect("navigation");
    let target = feature.read_with(visual, |feature, _| {
        *feature.visible_pages().last().unwrap()
    });
    visual.update(|_, cx| feature.update(cx, |feature, cx| feature.select_page(target, cx)));
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    feature.read_with(visual, |feature, _| {
        assert!(feature.content_scroll.handle().offset().y < px(0.0));
        assert_eq!(feature.page, target);
    });
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    feature.read_with(visual, |feature, _| assert_eq!(feature.page, target));
    assert_eq!(visual.debug_bounds("main-settings-nav"), Some(nav));
    let position = feature.read_with(visual, |feature, _| {
        feature.content_scroll.handle().bounds().center()
    });
    visual.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(10000.0))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    feature.read_with(visual, |feature, _| {
        assert_eq!(feature.page, feature.visible_pages()[0]);
    });
    // スクロールバーのドラッグは wheel event を発行しない。
    visual.update(|window, cx| {
        let handle = feature.read(cx).content_scroll.handle();
        let max = crate::gpui_compat::max_offset(&handle).height;
        handle.set_offset(gpui::point(px(0.0), -max / 2.0));
        window.refresh();
        window.draw(cx).clear();
    });
    feature.read_with(visual, |feature, _| {
        let pages = feature.visible_pages();
        let index = feature
            .content_scroll
            .handle()
            .top_item()
            .min(pages.len() - 1);
        assert_eq!(feature.page, pages[index]);
    });
}
impl SettingsRepository for MemorySettings {
    fn load(&self) -> Result<SettingsDocument, SettingsError> {
        Ok(SettingsDocument::empty(SettingsScope::User))
    }
    fn save(&self, _: &SettingsDocument) -> Result<(), SettingsError> {
        Ok(())
    }
}

fn wait_for_settings(cx: &mut gpui::TestAppContext) {
    let coordinator = cx.update(|cx| SettingsCoordinator::shared(cx));
    let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(3);
    while cx.update(|cx| !coordinator.read(cx).test_is_idle()) {
        assert!(
            crate::time::Instant::now() < deadline,
            "settings queue did not drain"
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        cx.run_until_parked();
        std::thread::yield_now();
    }
}

#[gpui::test]
fn changes_from_different_windows_keep_the_other_theme_selection(cx: &mut gpui::TestAppContext) {
    let (first, second) = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        (cx.new(SettingsFeature::new), cx.new(SettingsFeature::new))
    });
    cx.run_until_parked();
    cx.update(|cx| {
        first.update(cx, |feature, cx| {
            feature.choose(settings::UI_THEME, SettingValue::text("lapis.white"), cx)
        });
        second.update(cx, |feature, cx| {
            feature.choose(
                settings::UI_SYNTAX_THEME,
                SettingValue::text(SyntaxTheme::Zed.as_id()),
                cx,
            )
        });
    });
    wait_for_settings(cx);
    cx.update(|cx| {
        assert_eq!(theme::active_id(cx).as_str(), "lapis.white");
        assert_eq!(theme::syntax::active_family(cx), SyntaxTheme::Zed);
    });
}

#[gpui::test]
fn two_features_observe_shared_theme_syntax_and_locale(cx: &mut gpui::TestAppContext) {
    let (first, second) = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        (cx.new(SettingsFeature::new), cx.new(SettingsFeature::new))
    });
    cx.run_until_parked();
    cx.update(|cx| {
        first.update(cx, |feature, cx| {
            feature.choose(settings::UI_THEME, SettingValue::text("lapis.white"), cx)
        });
        second.update(cx, |feature, cx| {
            feature.choose(
                settings::UI_SYNTAX_THEME,
                SettingValue::text(SyntaxTheme::Zed.as_id()),
                cx,
            )
        });
        first.update(cx, |feature, cx| {
            feature.choose(settings::UI_LOCALE, SettingValue::text("ja-JP"), cx)
        });
    });
    wait_for_settings(cx);
    cx.update(|cx| {
        for feature in [&first, &second] {
            let values = &feature.read(cx).values;
            assert_eq!(values.theme(), "lapis.white");
            assert_eq!(values.syntax_theme(), Some(SyntaxTheme::Zed));
            assert_eq!(values.locale().as_str(), "ja-JP");
        }
    });
}

#[gpui::test]
fn settings_navigation_renders_only_leaf_pages(cx: &mut gpui::TestAppContext) {
    let catalog = catalog_for_tests();
    let leaf = catalog
        .categories()
        .iter()
        .flat_map(|category| category.groups.iter())
        .flat_map(|group| group.pages.iter())
        .count();
    assert_eq!(leaf, catalog.pages().len());
    assert!(catalog.first_page().is_some());
    let _ = cx;
}

/// Host が公開した profile が Settings の候補になることを、描画と同じ経路で確かめる。
#[gpui::test]
fn default_terminal_profile_options_come_from_the_published_catalog(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        crate::features::settings_sources::SettingsSources::init(cx);
        // Terminal は Host の profile 一覧を設定の供給元として公開する。
        let catalog = crate::features::terminal::register_settings_source(cx);
        crate::features::terminal::publish_test_profile_catalog(
            &terminal::TerminalProfiles::new(
                vec![
                    terminal::TerminalProfile::new("powershell", "PowerShell", "powershell.exe"),
                    terminal::TerminalProfile::new("cmd", "Command Prompt", "cmd.exe"),
                ],
                Some(terminal::TerminalProfileId::new("powershell")),
            ),
            &catalog,
            cx,
        );
    });
    let feature = cx.update(|cx| cx.new(SettingsFeature::new));
    let definition = settings::catalog()
        .definition(settings::TERMINAL_DEFAULT_PROFILE)
        .expect("定義がある");
    feature.update(cx, |feature, cx| {
        let options = feature.options(definition, cx);
        let values = options
            .iter()
            .map(|(value, label)| {
                (
                    value.as_text().unwrap_or_default().to_owned(),
                    label.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                ("powershell".to_owned(), "PowerShell".to_owned()),
                ("cmd".to_owned(), "Command Prompt".to_owned()),
            ]
        );
        // 未保存の既定値は Host の既定 profile を選択状態にする。
        assert_eq!(
            feature
                .effective_value(definition, cx)
                .and_then(|value| value.as_text().map(ToOwned::to_owned)),
            Some("powershell".to_owned())
        );
    });
}

#[derive(Default)]
struct TestWeb(std::sync::Mutex<Option<String>>);
impl WebAccessControl for TestWeb {
    fn status(&self, _: &std::path::Path) -> Result<HostWebStatus, String> {
        Ok(HostWebStatus {
            url: self.0.lock().unwrap().clone(),
            addresses: vec!["127.0.0.1".into()],
            error: None,
        })
    }
    fn start(&self, _: &std::path::Path, _: &str, _: u16) -> Result<HostWebStatus, String> {
        *self.0.lock().unwrap() = Some("http://127.0.0.1:1234/".into());
        self.status(std::path::Path::new("."))
    }
    fn stop(&self, _: &std::path::Path) -> Result<HostWebStatus, String> {
        *self.0.lock().unwrap() = None;
        self.status(std::path::Path::new("."))
    }
    fn invite(&self, _: &std::path::Path) -> Result<zeroize::Zeroizing<String>, String> {
        Err("not used".into())
    }
    fn devices(&self, _: &std::path::Path) -> Result<Vec<HostDevice>, String> {
        Ok(Vec::new())
    }
    fn revoke(&self, _: &std::path::Path, _: &str) -> Result<(), String> {
        Err("not used".into())
    }
}

#[gpui::test]
fn web_controls_refresh_after_start_and_stop(cx: &mut gpui::TestAppContext) {
    let feature = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        SettingsCoordinator::init(
            desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap(),
            cx,
        );
        let control: Arc<dyn WebAccessControl> = Arc::new(TestWeb::default());
        let feature = cx.new(|cx| SettingsFeature::with_web_access(cx, None, Some(control)));
        feature.update(cx, |feature, cx| {
            feature.set_workspace(Some(PathBuf::from("workspace")), None, cx);
            feature.select_page("host.web", cx);
        });
        feature
    });
    let wait = |cx: &mut gpui::TestAppContext| {
        let deadline = crate::time::Instant::now() + std::time::Duration::from_secs(5);
        while cx.update(|cx| feature.read(cx).web.as_ref().unwrap().busy) {
            assert!(crate::time::Instant::now() < deadline);
            cx.run_until_parked();
            std::thread::yield_now();
        }
    };
    wait(cx);
    cx.update(|cx| {
        feature.update(cx, |feature, cx| {
            feature.web_request(WebOperation::Start("127.0.0.1".into()), cx)
        })
    });
    wait(cx);
    cx.update(|cx| {
        assert_eq!(
            feature
                .read(cx)
                .web
                .as_ref()
                .unwrap()
                .status
                .as_ref()
                .unwrap()
                .url
                .as_deref(),
            Some("http://127.0.0.1:1234/")
        )
    });
    cx.update(|cx| {
        feature.update(cx, |feature, cx| {
            feature.web_request(WebOperation::Stop, cx)
        })
    });
    wait(cx);
    cx.update(|cx| {
        assert_eq!(
            feature
                .read(cx)
                .web
                .as_ref()
                .unwrap()
                .status
                .as_ref()
                .unwrap()
                .url,
            None
        )
    });
}

#[cfg(debug_assertions)]
#[gpui::test]
fn web_management_page_can_be_opened_from_settings_navigation(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        SettingsCoordinator::init(
            desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap(),
            cx,
        );
    });
    let (feature, visual) = {
        let control: Arc<dyn WebAccessControl> = Arc::new(TestWeb::default());
        cx.add_window_view(|_, cx| SettingsFeature::with_web_access(cx, None, Some(control)))
    };
    feature.read_with(visual, |feature, _| {
        assert!(feature.web.is_some(), "Web service missing")
    });
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    assert!(
        visual.debug_bounds("main-settings-nav").is_some(),
        "settings navigation missing"
    );
    visual.update(|window, cx| {
        feature.read(cx).nav_scroll.handle().scroll_to_bottom();
        window.refresh();
        window.draw(cx).clear();
    });
    let button = visual
        .debug_bounds("settings-host-web")
        .expect("Web navigation");
    visual.simulate_click(button.center(), gpui::Modifiers::none());
    feature.read_with(visual, |feature, _| assert_eq!(feature.page, "host.web"));
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    assert!(visual.debug_bounds("web-start").is_some());
    assert!(visual.debug_bounds("web-invite").is_some());
}

fn web_addresses() -> Vec<desktop_client::WebAddress> {
    vec![
        desktop_client::WebAddress::new(desktop_client::WebAddressKind::Local, "127.0.0.1"),
        desktop_client::WebAddress::new(desktop_client::WebAddressKind::Network, "192.168.1.20"),
        desktop_client::WebAddress::new(desktop_client::WebAddressKind::Tailscale, "100.64.1.23"),
    ]
}

fn selected_web_address(feature: &SettingsFeature) -> Option<desktop_client::WebAddress> {
    let access = feature.web_access()?.with_loopback_fallback();
    feature.selected_web_address(&access)
}

fn web_feature(
    cx: &mut gpui::TestAppContext,
    addresses: Vec<desktop_client::WebAddress>,
    workspace_id: Option<String>,
) -> gpui::Entity<SettingsFeature> {
    cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        let source: crate::app::WebAccessSource = Arc::new(move || {
            Some(desktop_client::WebAccess::new(
                4747,
                addresses.clone(),
                workspace_id.clone(),
            ))
        });
        cx.new(|cx| SettingsFeature::with_web_access(cx, Some(source), None))
    })
}

/// 更新の項目が定義データだけで描画され、配布元の場所を編集して保存できることを確かめる。
#[gpui::test]
fn update_settings_render_from_the_catalog_and_save_the_location(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
    });
    let feature = cx.update(|cx| cx.new(SettingsFeature::new));
    feature.update(cx, |feature, cx| {
        // 更新ページには定義済みの 3 項目だけが並ぶ。
        let keys = catalog_for_tests()
            .settings_of("update.channel")
            .map(|definition| definition.key)
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                settings::UPDATE_MODE,
                settings::UPDATE_SOURCE,
                settings::UPDATE_SOURCE_LOCATION
            ]
        );
        // 候補を持つ項目は定義の候補から、持たない項目は入力を出す。
        let location = settings::catalog()
            .definition(settings::UPDATE_SOURCE_LOCATION)
            .expect("定義がある");
        assert!(feature.options(location, cx).is_empty());
        assert_eq!(location.value_kind, settings::ValueKind::Text);
        assert_eq!(
            feature.values.update_mode(),
            settings::UpdateMode::Auto,
            "既定は自動"
        );
    });
}

/// 取得元を変えたら以前の location を流用しない。場所の意味は取得元ごとに変わる。
/// 同じ取得元の再選択では保存済みの場所を残す。
#[gpui::test]
fn changing_the_update_source_clears_the_previous_location(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
    });
    let feature = cx.update(|cx| cx.new(SettingsFeature::new));

    // まず Local と場所を保存し、保存が反映されるまで待つ。
    feature.update(cx, |feature, cx| {
        feature.choose(settings::UPDATE_SOURCE, SettingValue::text("local"), cx);
    });
    wait_for_settings(cx);
    feature.update(cx, |feature, cx| {
        feature.choose(
            settings::UPDATE_SOURCE_LOCATION,
            SettingValue::text("C:/releases"),
            cx,
        );
    });
    wait_for_settings(cx);
    cx.update(|cx| {
        let values = &feature.read(cx).values;
        assert_eq!(values.update_source(), settings::UpdateSource::Local);
        assert_eq!(
            values.update_source_location().as_deref(),
            Some("C:/releases")
        );
    });

    // 同じ Local を選び直しても場所は消さない。
    feature.update(cx, |feature, cx| {
        feature.choose(settings::UPDATE_SOURCE, SettingValue::text("local"), cx);
    });
    wait_for_settings(cx);
    cx.update(|cx| {
        assert_eq!(
            feature.read(cx).values.update_source_location().as_deref(),
            Some("C:/releases"),
            "取得元が変わっていなければ場所を残す"
        );
    });

    // Server へ変えたら場所を消し、再入力を促す。
    feature.update(cx, |feature, cx| {
        feature.choose(settings::UPDATE_SOURCE, SettingValue::text("server"), cx);
    });
    wait_for_settings(cx);
    cx.update(|cx| {
        let values = &feature.read(cx).values;
        assert_eq!(values.update_source(), settings::UpdateSource::Server);
        assert_eq!(values.update_source_location(), None);
    });
}

/// 接続ページは保存値ではなく、いま集めた候補を表示する。
#[gpui::test]
fn connection_page_shows_the_current_web_addresses(cx: &mut gpui::TestAppContext) {
    let feature = web_feature(cx, web_addresses(), Some("workspace-1".to_owned()));
    cx.update(|cx| {
        feature.update(cx, |feature, cx| {
            feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
        });
        let access = feature.read(cx).web_access().expect("接続情報がある");
        assert_eq!(access.addresses().len(), 3);
        let selected = selected_web_address(feature.read(cx)).expect("候補がある");
        let access = feature.read(cx).web_access().expect("接続情報がある");
        assert_eq!(selected.host, "127.0.0.1");
        assert_eq!(
            access.page_url(Some(&selected)).as_deref(),
            Some("http://127.0.0.1:4747/")
        );
    });
}

/// 候補を選ぶと、コピーと「ブラウザーで開く」の対象がその候補へ変わる。
#[gpui::test]
fn choosing_a_web_address_changes_the_urls_to_open_and_copy(cx: &mut gpui::TestAppContext) {
    let feature = web_feature(cx, web_addresses(), None);
    cx.update(|cx| {
        feature.update(cx, |feature, cx| {
            feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
            feature.select_web_host("100.64.1.23".to_owned(), cx);
        });
        let selected = selected_web_address(feature.read(cx)).expect("候補がある");
        let access = feature.read(cx).web_access().expect("接続情報がある");
        assert_eq!(selected.host, "100.64.1.23");
        assert_eq!(
            access.page_url(Some(&selected)).as_deref(),
            Some("http://100.64.1.23:4747/")
        );
    });
}

/// Tailscale の IP が変わっても、表示は供給元の現在値へ追随する。
#[gpui::test]
fn web_addresses_follow_the_current_network(cx: &mut gpui::TestAppContext) {
    let current = std::sync::Arc::new(std::sync::Mutex::new("100.64.1.23".to_owned()));
    let observed = current.clone();
    let feature = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        let source: crate::app::WebAccessSource = Arc::new(move || {
            let host = observed.lock().unwrap().clone();
            Some(desktop_client::WebAccess::new(
                4747,
                vec![
                    desktop_client::WebAddress::new(
                        desktop_client::WebAddressKind::Local,
                        "127.0.0.1",
                    ),
                    desktop_client::WebAddress::new(
                        desktop_client::WebAddressKind::Tailscale,
                        host,
                    ),
                ],
                None,
            ))
        });
        cx.new(|cx| SettingsFeature::with_web_access(cx, Some(source), None))
    });
    cx.update(|cx| {
        feature.update(cx, |feature, cx| {
            feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
            feature.select_web_host("100.64.1.23".to_owned(), cx);
        });
        *current.lock().unwrap() = "100.64.9.9".to_owned();
        // 保存値ではないため、選んだホストが消えていれば現在の候補へ戻る。
        let selected = selected_web_address(feature.read(cx)).expect("候補がある");
        assert_eq!(selected.host, "127.0.0.1");
        let access = feature.read(cx).web_access().expect("接続情報がある");
        assert!(
            access
                .addresses()
                .iter()
                .any(|address| address.host == "100.64.9.9")
        );
    });
}

/// Desktop は接続中の Workspace ID を初回入力の候補として表示する。
#[gpui::test]
fn connection_page_shows_the_connected_workspace_id(cx: &mut gpui::TestAppContext) {
    let feature = web_feature(cx, web_addresses(), None);
    cx.update(|cx| {
        feature.update(cx, |feature, cx| {
            feature.set_workspace(None, Some("workspace-9".to_owned()), cx);
            feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx);
        });
        assert_eq!(
            feature
                .read(cx)
                .web_access()
                .and_then(|access| access.workspace_id)
                .or_else(|| feature.read(cx).workspace_id.clone()),
            Some("workspace-9".to_owned())
        );
    });
}

/// 接続情報を出せない構成では、空のページをナビへ出さない。
#[gpui::test]
fn connection_page_is_hidden_without_a_web_source(cx: &mut gpui::TestAppContext) {
    let feature = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        cx.new(SettingsFeature::new)
    });
    let page = settings::catalog()
        .page(settings::catalog::CONNECTION_WEB_PAGE)
        .expect("接続ページの定義がある");
    cx.update(|cx| {
        assert!(!feature.read(cx).page_available(page));
    });
    let available = web_feature(cx, web_addresses(), None);
    cx.update(|cx| {
        assert!(available.read(cx).page_available(page));
    });
}

/// 供給元はあっても取得結果が `None` なら、空のページをナビへ出さない。
#[gpui::test]
fn connection_page_is_hidden_when_the_source_returns_none(cx: &mut gpui::TestAppContext) {
    let feature = cx.update(|cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        let source: crate::app::WebAccessSource = Arc::new(|| None);
        cx.new(|cx| SettingsFeature::with_web_access(cx, Some(source), None))
    });
    let page = settings::catalog()
        .page(settings::catalog::CONNECTION_WEB_PAGE)
        .expect("接続ページの定義がある");
    cx.update(|cx| {
        assert!(feature.read(cx).web_access().is_none());
        assert!(!feature.read(cx).page_available(page));
    });
}

/// 描画した接続ページのボタンが、実際にコピーとブラウザー起動を行う。
#[gpui::test]
fn connection_page_buttons_copy_and_open_the_selected_address(cx: &mut gpui::TestAppContext) {
    use gpui::Modifiers;

    let (feature, visual) = cx.add_window_view(|_, cx| {
        theme::init(&ThemeId::new("lapis.dark"), None, cx);
        let settings = desktop_client::SettingsSession::load(Arc::new(MemorySettings)).unwrap();
        SettingsCoordinator::init(settings, cx);
        let source: crate::app::WebAccessSource = std::sync::Arc::new(|| {
            Some(desktop_client::WebAccess::new(
                4747,
                vec![
                    desktop_client::WebAddress::new(
                        desktop_client::WebAddressKind::Local,
                        "127.0.0.1",
                    ),
                    desktop_client::WebAddress::new(
                        desktop_client::WebAddressKind::Tailscale,
                        "100.64.1.23",
                    ),
                ],
                None,
            ))
        });
        SettingsFeature::with_web_access(cx, Some(source), None)
    });
    feature.update(visual, |feature, cx| {
        feature.select_page(settings::catalog::CONNECTION_WEB_PAGE, cx)
    });
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });

    // Tailscale の候補を選ぶと、コピーと起動の対象がその候補へ変わる。
    let tailscale = visual
        .debug_bounds("web-address-100.64.1.23")
        .expect("候補が出ている");
    visual.simulate_click(tailscale.center(), Modifiers::none());
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    let copy = visual
        .debug_bounds("web-page-url-copy")
        .expect("ページ URL のコピー操作が出ている");
    visual.simulate_click(copy.center(), Modifiers::none());
    assert_eq!(
        visual.read_from_clipboard().and_then(|item| item.text()),
        Some("http://100.64.1.23:4747/".to_owned())
    );

    let open = visual
        .debug_bounds("web-open-browser")
        .expect("ブラウザーで開く操作が出ている");
    visual.simulate_click(open.center(), Modifiers::none());
    assert_eq!(
        visual.opened_url().as_deref(),
        Some("http://100.64.1.23:4747/")
    );
}
