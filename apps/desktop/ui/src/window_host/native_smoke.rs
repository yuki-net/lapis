//! 隔離したWorkspaceでNative Windowの寿命・所属・共有状態を検証する。
use super::*;

pub(crate) fn run_native_smoke(
    root: &mut DesktopRoot,
    window: &mut Window,
    cx: &mut Context<DesktopRoot>,
) {
    let id = root.editor.read(cx).document_id().unwrap();
    let shared_path = root.editor.read(cx).document().path().unwrap().to_owned();
    let expected_documents = root
        .group
        .read(cx)
        .latest
        .as_ref()
        .unwrap()
        .tabs
        .iter()
        .map(|tab| tab.id.clone())
        .collect::<Vec<_>>();
    let handle = window.window_handle();
    cx.spawn(async move |this, cx| {
        // 起動直後の描画とidleのCPUサンプルを分ける。
        gpui::Timer::after(std::time::Duration::from_millis(500)).await;
        eprintln!("SMOKE baseline sample begin");
        gpui::Timer::after(std::time::Duration::from_secs(5)).await;
        // UI と同じ on_drag → Escape / Window 外 MouseUp を通す。
        #[cfg(target_os = "windows")]
        {
            let shell = this.update(cx, |root, _| root.shell.clone()).unwrap();
            simulate_tab_drag(&shell, handle, true, None, cx).await;
            this.update(cx, |root, cx| {
                assert_eq!(root.group.read(cx).members.len(), 1);
                assert!(root.group.read(cx).documents(ROOT_HOST).contains(&id));
                eprintln!("SMOKE Escape keeps source tab OK");
            })
            .unwrap();
            simulate_tab_drag(&shell, handle, false, None, cx).await;
        }
        #[cfg(not(target_os = "windows"))]
        this.update(cx, |root, cx| {
            root.detach_tab(0, PanelTab::Document(id.clone()), cx)
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            assert_eq!(root.group.read(cx).members.len(), 2);
            eprintln!("SMOKE drag outside creates native window OK");
        })
        .unwrap();
        gpui::Timer::after(std::time::Duration::from_millis(500)).await;
        eprintln!("SMOKE idle sample begin");
        gpui::Timer::after(std::time::Duration::from_secs(5)).await;
        #[cfg(target_os = "windows")]
        verify_drag_routes(&this, handle, &id, cx).await;
        handle
            .update(cx, |_, window, cx| {
                this.update(cx, |root, cx| {
                    root.command(UiCommand::NewWindow, window, cx)
                })
            })
            .unwrap()
            .unwrap();
        this.update(cx, |root, cx| {
            assert_eq!(root.group.read(cx).members.len(), 2);
            let child = *root
                .group
                .read(cx)
                .members
                .keys()
                .find(|id| **id != 0)
                .unwrap();
            assert_eq!(root.group.read(cx).documents(child), vec![id.clone()]);
            let child_editor = root.group.read(cx).members[&child].editor.clone();
            assert_eq!(child_editor.read(cx).document_id(), Some(id.clone()));
            assert_ne!(root.editor.read(cx).document_id(), Some(id.clone()));
            child_editor.update(cx, |editor, cx| editor.replace_selection("smoke edit ", cx));
            eprintln!("SMOKE ownership and independent active documents OK");
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            let editor = root
                .group
                .read(cx)
                .members
                .values()
                .find(|member| member.editor.entity_id() != root.editor.entity_id())
                .unwrap()
                .editor
                .clone();
            editor.update(cx, |editor, cx| {
                editor.submit(true, |_| Err("smoke recovery".into()), cx);
            });
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            let editor = root
                .group
                .read(cx)
                .members
                .values()
                .find(|member| member.editor.entity_id() != root.editor.entity_id())
                .unwrap()
                .editor
                .clone();
            assert!(editor.read(cx).needs_recovery());
            editor.update(cx, |editor, cx| editor.recover_draft(cx));
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            let child = *root
                .group
                .read(cx)
                .members
                .keys()
                .find(|host| **host != ROOT_HOST)
                .unwrap();
            let recovered = root
                .group
                .read(cx)
                .documents(child)
                .into_iter()
                .find(|candidate| candidate != &id)
                .expect("recovered draft belongs to the child");
            let snapshot = root.group.read(cx).latest.as_ref().unwrap();
            assert!(
                snapshot
                    .documents
                    .iter()
                    .find(|cache| cache.id == recovered)
                    .unwrap()
                    .document
                    .content_for_save()
                    .contains("smoke edit")
            );
            let editor = root.group.read(cx).members[&child].editor.clone();
            assert_eq!(editor.read(cx).document_id(), Some(recovered.clone()));
            editor.update(cx, |editor, cx| {
                editor.submit(
                    false,
                    move |client| {
                        client
                            .editor
                            .close_document(
                                &recovered,
                                lapis_desktop_client::DocumentCloseDisposition::DiscardChanges,
                            )
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            });
            eprintln!("SMOKE child recovery preserves its draft and ownership OK");
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        let (child, handle) = this
            .update(cx, |root, cx| {
                let snapshot = root.group.read(cx).latest.as_ref().unwrap();
                assert!(
                    snapshot
                        .documents
                        .iter()
                        .find(|doc| doc.id == id)
                        .unwrap()
                        .document
                        .content_for_save()
                        .contains("smoke edit")
                );
                let child = *root
                    .group
                    .read(cx)
                    .members
                    .keys()
                    .find(|id| **id != 0)
                    .unwrap();
                root.transfer_tab(
                    child,
                    0,
                    PanelPosition::Main,
                    PanelTab::Document(id.clone()),
                    cx,
                );
                eprintln!("SMOKE shared edited content and redock OK");
                let handle = root.group.read(cx).members[&child].handle;
                let editor = root.group.read(cx).members[&child].editor.clone();
                // UIへの応答前にcloseを要求しても、新しいタブを子に残さない。
                editor.update(cx, |editor, cx| {
                    editor.submit(
                        false,
                        |client| {
                            client.editor.new_document();
                            Ok(())
                        },
                        cx,
                    );
                });
                (child, handle)
            })
            .unwrap();
        handle
            .update(cx, |_, window, cx| {
                this.update(cx, |root, cx| root.close_hosts(child, None, window, cx))
            })
            .unwrap()
            .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            assert_eq!(root.group.read(cx).members.len(), 1);
            assert_eq!(
                root.group.read(cx).latest.as_ref().unwrap().tabs.len(),
                expected_documents.len()
            );
            assert!(
                root.group
                    .read(cx)
                    .latest
                    .as_ref()
                    .unwrap()
                    .tabs
                    .iter()
                    .all(|tab| expected_documents.contains(&tab.id))
            );
            assert!(
                root.group
                    .read(cx)
                    .latest
                    .as_ref()
                    .unwrap()
                    .tabs
                    .iter()
                    .all(|tab| root.group.read(cx).documents(ROOT_HOST).contains(&tab.id))
            );
            assert!(root.group.read(cx).documents(0).contains(&id));
            root.detach_tab(0, PanelTab::Document(id.clone()), cx);
            eprintln!("SMOKE empty child close and regeneration OK");
            let child = *root
                .group
                .read(cx)
                .members
                .keys()
                .find(|id| **id != 0)
                .unwrap();
            let editor = root.group.read(cx).members[&child].editor.clone();
            editor.update(cx, |editor, cx| {
                editor.submit(
                    false,
                    |client| {
                        client
                            .editor
                            .save_document()
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            });
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        let (handle, peer) = this
            .update(cx, |root, cx| {
                let own_id = cx.entity_id();
                let own_group = root.group.entity_id();
                let mut peer = None;
                for handle in cx.windows() {
                    let found = handle
                        .update(cx, |view, window, cx| {
                            let Ok(entity) = view.downcast::<DesktopRoot>() else {
                                return None;
                            };
                            if entity.entity_id() == own_id {
                                return None;
                            }
                            entity.update(cx, |other, cx| {
                                assert_ne!(other.group.entity_id(), own_group);
                                let before = other.group.read(cx).layout.tabs.len();
                                other.host_command(
                                    0,
                                    UiCommand::MoveTab(
                                        crate::shell::DraggedPanelTab {
                                            group: Some(own_group),
                                            source_host: 0,
                                            source_panel: PanelPosition::Main,
                                            tab: PanelTab::Document(id.clone()),
                                        },
                                        PanelPosition::Main,
                                    ),
                                    window,
                                    cx,
                                );
                                assert_eq!(other.group.read(cx).layout.tabs.len(), before);
                            });
                            Some(entity)
                        })
                        .unwrap();
                    if found.is_some() {
                        peer = found;
                    }
                }
                (
                    root.group.read(cx).members[&0].handle,
                    peer.expect("independent root B"),
                )
            })
            .unwrap();
        #[cfg(target_os = "windows")]
        {
            let (source, source_shell, owner) = this
                .update(cx, |root, cx| {
                    let owner = root.group.read(cx).layout.tabs[&PanelTab::Document(id.clone())].0;
                    let member = &root.group.read(cx).members[&owner];
                    (member.handle, member.shell.clone(), owner)
                })
                .unwrap();
            let (target, target_shell) = peer
                .update(cx, |root, cx| {
                    (
                        root.group.read(cx).members[&ROOT_HOST].handle,
                        root.shell.clone(),
                    )
                })
                .unwrap();
            let point = show_drop_target(target, &target_shell, cx);
            simulate_tab_drag(&source_shell, source, false, Some(point), cx).await;
            wait_for(&this, cx, None).await;
            this.update(cx, |root, cx| {
                assert_eq!(
                    root.group.read(cx).layout.tabs[&PanelTab::Document(id.clone())].0,
                    owner
                );
                assert_eq!(root.group.read(cx).members.len(), 2);
                eprintln!("SMOKE native foreign Window drop keeps source tab OK");
            })
            .unwrap();
        }
        handle
            .update(cx, |_, window, cx| {
                this.update(cx, |root, cx| root.close_hosts(0, None, window, cx))
            })
            .unwrap()
            .unwrap();
        wait_for(&peer.downgrade(), cx, Some(1)).await;
        let handle = peer
            .update(cx, |other, cx| {
                assert_eq!(other.group.read(cx).members.len(), 1);
                assert_eq!(cx.windows().len(), 1);
                other.group.read(cx).members[&0].handle
            })
            .unwrap();
        eprintln!("SMOKE independent group, foreign transfer rejection and root cascade OK");
        let peer_path = shared_path.clone();
        peer.update(cx, |other, cx| {
            other.editor.update(cx, |editor, cx| {
                editor.submit(
                    false,
                    move |client| {
                        client
                            .editor
                            .open_workspace(peer_path.parent().unwrap().to_owned())
                            .map_err(|e| e.to_string())?;
                        client
                            .editor
                            .open_path(peer_path)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            });
        })
        .unwrap();
        wait_for(&peer.downgrade(), cx, None).await;
        handle
            .update(cx, |_, window, cx| {
                peer.update(cx, |other, cx| {
                    let id = other.editor.read(cx).document_id().unwrap();
                    other.detach_tab(0, PanelTab::Document(id), cx);
                    other.command(UiCommand::NewWindow, window, cx);
                })
            })
            .unwrap();
        wait_for(&peer.downgrade(), cx, Some(3)).await;
        let shared_peer = peer
            .update(cx, |_, cx| {
                let own = cx.entity_id();
                cx.windows()
                    .into_iter()
                    .find_map(|window| {
                        window
                            .update(cx, |view, _, cx| {
                                let entity = view.downcast::<DesktopRoot>().ok()?;
                                if entity.entity_id() == own {
                                    return None;
                                }
                                let path = shared_path.clone();
                                entity.update(cx, |other, cx| {
                                    other.editor.update(cx, |editor, cx| {
                                        editor.submit(
                                            false,
                                            move |client| {
                                                client
                                                    .editor
                                                    .open_workspace(
                                                        path.parent().unwrap().to_owned(),
                                                    )
                                                    .map_err(|e| e.to_string())?;
                                                client
                                                    .editor
                                                    .open_path(path)
                                                    .map_err(|e| e.to_string())
                                            },
                                            cx,
                                        );
                                    })
                                });
                                Some(entity)
                            })
                            .unwrap()
                    })
                    .unwrap()
            })
            .unwrap();
        wait_for(&shared_peer.downgrade(), cx, Some(3)).await;
        shared_peer
            .update(cx, |other, cx| {
                assert_eq!(other.editor.read(cx).document_id(), Some(id.clone()))
            })
            .unwrap();
        handle
            .update(cx, |_, window, cx| {
                peer.update(cx, |other, cx| other.close_application(window, cx))
            })
            .unwrap();
        eprintln!("SMOKE Quit of multiple groups requested; process must exit");
        eprintln!("WINDOW_HOST_SMOKE_PASS");
    })
    .detach();
}

#[cfg(target_os = "windows")]
async fn simulate_tab_drag(
    shell: &Entity<Shell>,
    handle: gpui::AnyWindowHandle,
    cancel: bool,
    destination: Option<drag::ScreenPoint>,
    cx: &mut gpui::AsyncApp,
) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Graphics::Gdi::ScreenToClient,
        UI::WindowsAndMessaging::{
            PostMessageW, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        },
    };
    let original_window_count = cx.update(|cx| cx.windows().len()).unwrap();
    let (hwnd, start, mut end) = handle
        .update(cx, |_, window, cx| {
            window.draw(cx).clear();
            let bounds = shell.read(cx).panel_bounds.borrow()[&PanelPosition::Main];
            if shell.read(cx).detached {
                assert!(
                    bounds.size.height > window.viewport_size().height - gpui::px(100.),
                    "Detached panel must fill its Window"
                );
            }
            let scale = window.scale_factor();
            let start = bounds.origin + gpui::point(gpui::px(50.), gpui::px(17.));
            let pack = |x: f32, y: f32| {
                LPARAM(
                    (((y * scale).round() as i16 as u16 as u32) << 16
                        | (x * scale).round() as i16 as u16 as u32) as isize,
                )
            };
            let raw = HasWindowHandle::window_handle(window).unwrap().as_raw();
            let RawWindowHandle::Win32(raw) = raw else {
                panic!("Windows handle required");
            };
            (
                raw.hwnd.get(),
                pack(f32::from(start.x), f32::from(start.y)),
                pack(f32::from(window.viewport_size().width) + 60., 80.),
            )
        })
        .unwrap();
    let hwnd = HWND(hwnd as *mut _);
    if let Some(destination) = destination {
        let mut point = windows::Win32::Foundation::POINT {
            x: destination.x,
            y: destination.y,
        };
        assert!(unsafe { ScreenToClient(hwnd, &mut point) }.as_bool());
        end =
            LPARAM(((point.y as i16 as u16 as u32) << 16 | point.x as i16 as u16 as u32) as isize);
    }
    // smoke モードの自プロセス Window のみにイベントを送る。
    unsafe { PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(1), start) }.unwrap();
    gpui::Timer::after(std::time::Duration::from_millis(60)).await;
    let moved = LPARAM(start.0 + 20);
    unsafe { PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(1), moved) }.unwrap();
    gpui::Timer::after(std::time::Duration::from_millis(60)).await;
    cx.update(|cx| {
        assert!(cx.has_active_drag(), "Tab header did not start a drag");
        assert_eq!(
            cx.windows().len(),
            original_window_count + 1,
            "Native drag preview window was not opened"
        );
    })
    .unwrap();
    if cancel {
        unsafe { PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(0x1b), LPARAM(1)) }.unwrap();
        unsafe {
            PostMessageW(
                Some(hwnd),
                WM_KEYUP,
                WPARAM(0x1b),
                LPARAM(0xc0000001_u32 as isize),
            )
        }
        .unwrap();
        gpui::Timer::after(std::time::Duration::from_millis(60)).await;
        cx.update(|cx| {
            assert!(!cx.has_active_drag());
            assert_eq!(cx.windows().len(), original_window_count);
        })
        .unwrap();
    }
    unsafe { PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), end) }.unwrap();
    gpui::Timer::after(std::time::Duration::from_millis(100)).await;
    cx.update(|cx| {
        assert!(!cx.has_active_drag());
        assert_eq!(
            cx.windows().len(),
            original_window_count + usize::from(!cancel && destination.is_none()),
            "Preview was not closed when the drag ended"
        );
    })
    .unwrap();
}

#[cfg(target_os = "windows")]
fn show_drop_target(
    handle: gpui::AnyWindowHandle,
    shell: &Entity<Shell>,
    cx: &mut gpui::AsyncApp,
) -> drag::ScreenPoint {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            HWND_TOP, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
            ShowWindow,
        },
    };
    handle
        .update(cx, |_, window, cx| {
            window.draw(cx).clear();
            let position = shell.read(cx).panel_bounds.borrow()[&PanelPosition::Main].center();
            let RawWindowHandle::Win32(raw) =
                HasWindowHandle::window_handle(window).unwrap().as_raw()
            else {
                panic!("Windows handle required")
            };
            let hwnd = HWND(raw.hwnd.get() as *mut _);
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            }
            .unwrap();
            let point = drag::screen_position(position, window).unwrap();
            assert!(
                drag::is_source_window(window, point),
                "Native hit test did not select the fixture window"
            );
            point
        })
        .unwrap()
}

#[cfg(target_os = "windows")]
async fn verify_drag_routes(
    root: &WeakEntity<DesktopRoot>,
    handle: gpui::AnyWindowHandle,
    id: &DocumentId,
    cx: &mut gpui::AsyncApp,
) {
    let (first, second, first_handle, first_shell, second_handle, second_shell, root_shell) = root
        .update(cx, |root, cx| {
            let first = root.group.read(cx).layout.tabs[&PanelTab::Document(id.clone())].0;
            let second = root.group.update(cx, |group, _| group.layout.add_window());
            let group = root.group.clone();
            let weak_root = cx.entity().downgrade();
            let worker = root.worker.clone();
            let options = crate::app::window_options(cx);
            cx.open_window(options, move |window, cx| {
                cx.new(|cx| WindowHost::new(group, second, weak_root, worker, window, cx))
            })
            .unwrap();
            let members = &root.group.read(cx).members;
            (
                first,
                second,
                members[&first].handle,
                members[&first].shell.clone(),
                members[&second].handle,
                members[&second].shell.clone(),
                root.shell.clone(),
            )
        })
        .unwrap();
    let point = show_drop_target(second_handle, &second_shell, cx);
    simulate_tab_drag(&first_shell, first_handle, false, Some(point), cx).await;
    wait_for(root, cx, None).await;
    root.update(cx, |root, cx| {
        assert_eq!(root.group.read(cx).documents(second), vec![id.clone()]);
        assert!(root.group.read(cx).documents(first).is_empty());
    })
    .unwrap();
    let point = show_drop_target(handle, &root_shell, cx);
    simulate_tab_drag(&second_shell, second_handle, false, Some(point), cx).await;
    wait_for(root, cx, None).await;
    root.update(cx, |root, cx| {
        assert!(root.group.read(cx).documents(ROOT_HOST).contains(id));
        assert_eq!(root.editor.read(cx).document_id().as_ref(), Some(id));
    })
    .unwrap();
    let point = show_drop_target(second_handle, &second_shell, cx);
    simulate_tab_drag(&root_shell, handle, false, Some(point), cx).await;
    wait_for(root, cx, None).await;
    root.update(cx, |root, cx| {
        assert_eq!(root.group.read(cx).documents(second), vec![id.clone()]);
        eprintln!("SMOKE sibling and overlapping Root/child drag redock OK");
    })
    .unwrap();
    first_handle
        .update(cx, |_, window, cx| {
            root.update(cx, |root, cx| root.close_hosts(first, None, window, cx))
                .unwrap();
        })
        .unwrap();
    wait_for(root, cx, Some(2)).await;
}

async fn wait_for(root: &WeakEntity<DesktopRoot>, cx: &mut gpui::AsyncApp, windows: Option<usize>) {
    for _ in 0..625 {
        gpui::Timer::after(std::time::Duration::from_millis(16)).await;
        if root
            .update(cx, |root, cx| {
                let group = root.group.read(cx);
                windows.is_none_or(|expected| cx.windows().len() == expected)
                    && !group.closing
                    && group
                        .members
                        .values()
                        .all(|member| !member.editor.read(cx).has_in_flight())
            })
            .unwrap()
        {
            return;
        }
    }
    panic!("Native Window smoke did not settle");
}
