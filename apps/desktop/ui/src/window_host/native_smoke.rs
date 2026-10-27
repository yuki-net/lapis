//! 隔離したWorkspaceでNative Windowの寿命・所属・共有状態を検証する。
use super::*;

struct SmokeFailConversation;

type SmokeSavedConversation = Option<lapis_desktop_client::ConversationState>;

struct SmokeRecordingConversation {
    inner: std::sync::Arc<dyn lapis_desktop_client::ConversationRepository>,
    saved: std::sync::Arc<std::sync::Mutex<SmokeSavedConversation>>,
}

impl lapis_desktop_client::ConversationRepository for SmokeRecordingConversation {
    fn load(
        &self,
    ) -> Result<lapis_desktop_client::ConversationState, lapis_workspace::WorkspaceError> {
        self.inner.load()
    }

    fn save(
        &self,
        expected: &lapis_desktop_client::ConversationState,
        projects: &[lapis_desktop_client::Project],
        records: &[lapis_desktop_client::ConversationRecord],
        active: &lapis_editor_core::ConversationId,
    ) -> Result<(), lapis_workspace::WorkspaceError> {
        self.inner.save(expected, projects, records, active)?;
        *self.saved.lock().unwrap() =
            Some((projects.to_vec(), records.to_vec(), Some(active.clone())));
        Ok(())
    }
}

impl lapis_desktop_client::ConversationRepository for SmokeFailConversation {
    fn load(
        &self,
    ) -> Result<lapis_desktop_client::ConversationState, lapis_workspace::WorkspaceError> {
        Ok(Default::default())
    }

    fn save(
        &self,
        _: &lapis_desktop_client::ConversationState,
        _: &[lapis_desktop_client::Project],
        _: &[lapis_desktop_client::ConversationRecord],
        _: &lapis_editor_core::ConversationId,
    ) -> Result<(), lapis_workspace::WorkspaceError> {
        Err(lapis_workspace::WorkspaceError::new(
            "smoke view persistence failure",
        ))
    }
}

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
        #[cfg(debug_assertions)]
        if std::env::var_os("LAPIS_INSPECTOR_SMOKE").is_some() {
            crate::devtools::native_smoke::run(handle, cx).await;
        }
        verify_panel_layout_resize(&this, handle, cx).await;
        #[cfg(target_os = "windows")]
        verify_panel_drag_resize(&this, handle, cx).await;
        eprintln!("SMOKE baseline sample begin");
        gpui::Timer::after(std::time::Duration::from_secs(5)).await;
        // UI と同じ on_drag → Escape / Window 外 MouseUp を通す。
        #[cfg(target_os = "windows")]
        {
            let shell = this.update(cx, |root, _| root.shell.clone()).unwrap();
            simulate_tab_drag(&this, &shell, handle, true, None, cx).await;
            this.update(cx, |root, cx| {
                assert_eq!(root.group.read(cx).members.len(), 1);
                assert!(root.group.read(cx).documents(ROOT_HOST).contains(&id));
                eprintln!("SMOKE Escape keeps source tab OK");
            })
            .unwrap();
            simulate_tab_drag(&this, &shell, handle, false, None, cx).await;
        }
        #[cfg(not(target_os = "windows"))]
        this.update(cx, |root, cx| {
            root.detach_tab(0, PanelTab::Document(id.clone()), cx)
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        #[cfg(debug_assertions)]
        if std::env::var_os("LAPIS_INSPECTOR_SMOKE").is_some() {
            let child = this
                .update(cx, |root, cx| {
                    root.group
                        .read(cx)
                        .members
                        .iter()
                        .find(|(host, _)| **host != ROOT_HOST)
                        .unwrap()
                        .1
                        .handle
                })
                .unwrap();
            crate::devtools::native_smoke::check_pair(handle, child, cx).await;
        }
        this.update(cx, |root, cx| {
            assert_eq!(root.group.read(cx).members.len(), 2);
            eprintln!("SMOKE drag outside creates native window OK");
        })
        .unwrap();
        gpui::Timer::after(std::time::Duration::from_millis(500)).await;
        eprintln!("SMOKE idle sample begin");
        gpui::Timer::after(std::time::Duration::from_secs(5)).await;
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
            let child = *root
                .group
                .read(cx)
                .members
                .keys()
                .find(|host| **host != ROOT_HOST)
                .unwrap();
            let worker = root.worker.clone();
            let _ = worker.request(|client| {
                client.edit_chain.restart();
                Ok(())
            });
            let editor = root.group.read(cx).members[&child].editor.clone();
            editor.update(cx, |editor, cx| {
                editor.replace_selection("view-only recovery ", cx);
            });
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        let (child, child_handle) = this
            .update(cx, |root, cx| {
                let child = *root
                    .group
                    .read(cx)
                    .members
                    .keys()
                    .find(|host| **host != ROOT_HOST)
                    .unwrap();
                let editor = root.group.read(cx).members[&child].editor.clone();
                assert!(editor.read(cx).needs_recovery());
                assert!(
                    editor
                        .read(cx)
                        .document()
                        .content_for_save()
                        .contains("smoke edit")
                );
                assert!(
                    !root
                        .group
                        .read(cx)
                        .latest
                        .as_ref()
                        .unwrap()
                        .documents
                        .iter()
                        .find(|cache| cache.id == id)
                        .unwrap()
                        .document
                        .content_for_save()
                        .contains("view-only recovery")
                );
                (child, root.group.read(cx).members[&child].handle)
            })
            .unwrap();
        child_handle
            .update(cx, |_, window, cx| {
                this.update(cx, |root, cx| {
                    root.group.update(cx, |group, cx| {
                        group.synchronization_required = true;
                        cx.notify();
                    });
                    root.close_hosts(child, None, window, cx);
                })
            })
            .unwrap()
            .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            let group = root.group.read(cx);
            assert!(!group.closing);
            assert!(!group.synchronization_required);
            assert!(group.documents(child).contains(&id));
            assert!(group.members[&child].editor.read(cx).needs_recovery());
            eprintln!("SMOKE Close retry preserves view-only recovery draft OK");
        })
        .unwrap();
        handle
            .update(cx, |_, window, cx| {
                this.update(cx, |root, cx| {
                    root.group.update(cx, |group, cx| {
                        group.synchronization_required = true;
                        cx.notify();
                    });
                    root.close_application(window, cx);
                })
            })
            .unwrap()
            .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            let group = root.group.read(cx);
            assert!(!group.closing);
            assert!(!group.synchronization_required);
            assert!(group.documents(child).contains(&id));
            assert!(group.members[&child].editor.read(cx).needs_recovery());
            eprintln!("SMOKE Quit retry preserves view-only recovery draft OK");
        })
        .unwrap();
        this.update(cx, |root, cx| {
            let editor = root.group.read(cx).members[&child].editor.clone();
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
                    .contains("view-only recovery")
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
        let (child, closing_child_handle) = this
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
                    ZonePosition::Main,
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
        // 永続化境界で失敗させ、実Runtimeのcloseと全Windowへの反映を検証する。
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(None));
        let original_conversation = this
            .update(cx, |root, _| {
                let recorded = recorded.clone();
                root.worker.request(move |client| {
                    let recording = lapis_desktop_client::ConversationSession::new(
                        std::sync::Arc::new(SmokeRecordingConversation {
                            inner: client.conversation.repository(),
                            saved: recorded.clone(),
                        }),
                        client.editor.snapshot(),
                    );
                    let original = std::mem::replace(&mut client.conversation, recording);
                    Ok((original, recorded))
                })
            })
            .unwrap();
        let (_original_conversation, recorded) = wait_client(original_conversation).await.unwrap();
        let barrier = this
            .update(cx, |root, _| root.worker.request(|_| Ok(())))
            .unwrap();
        wait_client(barrier).await.unwrap();
        *recorded.lock().unwrap() = None;
        this.update(cx, |root, cx| {
            root.group.update(cx, |group, cx| {
                group.synchronization_required = true;
                cx.notify();
            });
            root.shell.update(cx, |shell, cx| {
                shell.toggle_panel(ZonePosition::Bottom, cx);
            });
            root.schedule_persist(cx);
        })
        .unwrap();
        gpui::Timer::after(std::time::Duration::from_millis(700)).await;
        let barrier = this
            .update(cx, |root, _| root.worker.request(|_| Ok(())))
            .unwrap();
        wait_client(barrier).await.unwrap();
        assert!(recorded.lock().unwrap().is_none());
        let reconciled = this
            .update(cx, |root, _| {
                root.worker
                    .request(|client| Ok(client.reconcile_snapshot()))
            })
            .unwrap();
        let reconciled = wait_client(reconciled).await.unwrap();
        this.update(cx, |root, cx| {
            root.group
                .update(cx, |group, cx| group.publish_reconciled(reconciled, cx));
            root.schedule_persist(cx);
        })
        .unwrap();
        gpui::Timer::after(std::time::Duration::from_millis(700)).await;
        let barrier = this
            .update(cx, |root, _| root.worker.request(|_| Ok(())))
            .unwrap();
        wait_client(barrier).await.unwrap();
        let saved_view = recorded
            .lock()
            .unwrap()
            .clone()
            .expect("persistence must be retried after the synchronization latch is cleared");
        this.update(cx, |root, cx| {
            let expected = root.shell.read(cx).view_state();
            let active = saved_view
                .1
                .iter()
                .find(|record| Some(&record.id) == saved_view.2.as_ref())
                .expect("recorded active conversation");
            assert_eq!(active.view, expected);
            eprintln!("SMOKE deferred view persistence retries after reconciliation OK");
        })
        .unwrap();
        let saved_conversation = this
            .update(cx, |root, _| {
                root.worker.request(|client| {
                    let failing = lapis_desktop_client::ConversationSession::new(
                        std::sync::Arc::new(SmokeFailConversation),
                        client.editor.snapshot(),
                    );
                    let original = std::mem::replace(&mut client.conversation, failing);
                    Ok((original, client.snapshot()?))
                })
            })
            .unwrap();
        let (saved_conversation, stale_snapshot) = wait_client(saved_conversation).await.unwrap();
        closing_child_handle
            .update(cx, |_, window, cx| {
                this.update(cx, |root, cx| {
                    root.close_hosts(child, None, window, cx);
                    assert!(root.group.read(cx).closing);
                    root.close_hosts(child, None, window, cx);
                    root.close_application(window, cx);
                    assert!(root.group.read(cx).closing);
                    eprintln!("SMOKE concurrent Close and Quit rejected OK");
                })
            })
            .unwrap()
            .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            let group = root.group.read(cx);
            assert!(!group.closing);
            assert_eq!(group.members.len(), 2);
            assert!(group.documents(child).is_empty());
            assert!(
                group.members[&child]
                    .editor
                    .read(cx)
                    .document_id()
                    .is_none()
            );
            assert!(
                group
                    .latest
                    .as_ref()
                    .unwrap()
                    .document_owners
                    .values()
                    .all(|owner| *owner != child)
            );
            assert!(group.status.contains("smoke view persistence failure"));
            eprintln!("SMOKE committed close survives view persistence failure in all windows OK");
        })
        .unwrap();
        // closeより前の応答が遅れて届いても、ShellとEditorを過去へ戻さない。
        let closed = stale_snapshot
            .tabs
            .iter()
            .find(|tab| stale_snapshot.document_owners.get(&tab.id) == Some(&child))
            .unwrap()
            .id
            .clone();
        this.update(cx, |root, cx| {
            let editors = root
                .group
                .read(cx)
                .members
                .values()
                .map(|member| member.editor.clone())
                .collect::<Vec<_>>();
            for editor in editors {
                editor.update(cx, |_, cx| {
                    cx.emit(EditorEvent::Snapshot(Box::new(stale_snapshot.clone())));
                    cx.emit(EditorEvent::Idle);
                });
            }
        })
        .unwrap();
        wait_for(&this, cx, None).await;
        this.update(cx, |root, cx| {
            let group = root.group.read(cx);
            assert!(
                !group
                    .layout
                    .tabs
                    .contains_key(&PanelTab::Document(closed.clone()))
            );
            for member in group.members.values() {
                assert_ne!(member.editor.read(cx).document_id().as_ref(), Some(&closed));
                assert!(
                    member
                        .shell
                        .read(cx)
                        .shell
                        .panels()
                        .iter()
                        .all(|panel| !panel.contains(&PanelTab::Document(closed.clone())))
                );
            }
            eprintln!("SMOKE delayed responses keep all editors and shells synchronized OK");
        })
        .unwrap();
        let restored = this
            .update(cx, |root, _| {
                root.worker.request(move |client| {
                    client.conversation = saved_conversation;
                    Ok(())
                })
            })
            .unwrap();
        wait_client(restored).await.unwrap();
        #[cfg(debug_assertions)]
        let closing_inspector = std::env::var_os("LAPIS_INSPECTOR_SMOKE")
            .map(|_| crate::devtools::native_smoke::open_for_target(closing_child_handle, cx));
        closing_child_handle
            .update(cx, |_, window, cx| {
                this.update(cx, |root, cx| root.close_hosts(child, None, window, cx))
            })
            .unwrap()
            .unwrap();
        wait_for(&this, cx, None).await;
        #[cfg(debug_assertions)]
        if let Some(viewer) = closing_inspector {
            assert!(viewer.update(cx, |_, _, _| ()).is_err());
            eprintln!("INSPECTOR_NATIVE_TARGET_CLOSE_PASS");
        }
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
        // NewWindowはworker完了後に開くため、独立Root生成を待ってから検査する。
        wait_for(&this, cx, Some(3)).await;
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
                                            source_panel: crate::shell::PanelId::new(1),
                                            tab: PanelTab::Document(id.clone()),
                                        },
                                        crate::shell::PanelId::new(1),
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
            simulate_tab_drag(
                &this,
                &source_shell,
                source,
                false,
                Some((target, &target_shell)),
                cx,
            )
            .await;
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
        #[cfg(debug_assertions)]
        let peer_inspector = if std::env::var_os("LAPIS_INSPECTOR_SMOKE").is_some() {
            let targets = this
                .update(cx, |root, cx| {
                    root.group
                        .read(cx)
                        .members
                        .values()
                        .map(|member| member.handle)
                        .collect::<Vec<_>>()
                })
                .unwrap();
            for target in targets {
                crate::devtools::native_smoke::open_for_target(target, cx);
            }
            let target = peer
                .update(cx, |root, cx| {
                    root.group.read(cx).members[&ROOT_HOST].handle
                })
                .unwrap();
            Some(crate::devtools::native_smoke::open_for_target(target, cx))
        } else {
            None
        };
        #[cfg(not(debug_assertions))]
        let peer_inspector: Option<gpui::AnyWindowHandle> = None;
        let remaining_windows = 1 + usize::from(peer_inspector.is_some());
        handle
            .update(cx, |_, window, cx| {
                this.update(cx, |root, cx| root.close_hosts(0, None, window, cx))
            })
            .unwrap()
            .unwrap();
        wait_for(&peer.downgrade(), cx, Some(remaining_windows)).await;
        let handle = peer
            .update(cx, |other, cx| {
                assert_eq!(other.group.read(cx).members.len(), 1);
                assert_eq!(cx.windows().len(), remaining_windows);
                other.group.read(cx).members[&0].handle
            })
            .unwrap();
        eprintln!("SMOKE independent group, foreign transfer rejection and root cascade OK");
        if let Some(viewer) = peer_inspector {
            viewer
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
            wait_for(&peer.downgrade(), cx, Some(1)).await;
            eprintln!("INSPECTOR_NATIVE_ROOT_CASCADE_PASS");
        }
        let peer_path = shared_path.clone();
        peer.update(cx, |other, cx| {
            other.editor.update(cx, |editor, cx| {
                editor.submit(
                    false,
                    move |client| {
                        client
                            .conversation
                            .open_project(
                                &mut client.editor,
                                peer_path.parent().unwrap().to_owned(),
                            )
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
                                                    .conversation
                                                    .open_project(
                                                        &mut client.editor,
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
        #[cfg(all(target_os = "windows", debug_assertions))]
        if std::env::var_os("LAPIS_INSPECTOR_SMOKE").is_some() {
            crate::devtools::native_smoke::check_toolbar_hit_test_for_target(handle, cx).await;
        }
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

async fn verify_panel_layout_resize(
    root: &WeakEntity<DesktopRoot>,
    handle: gpui::AnyWindowHandle,
    cx: &mut gpui::AsyncApp,
) {
    let (original, document, saved, size) = handle
        .update(cx, |_, window, cx| {
            let (shell, document) = root
                .update(cx, |root, cx| {
                    (root.shell.clone(), root.editor.read(cx).document_id())
                })
                .unwrap();
            let original = shell.update(cx, |shell, _| {
                let original = shell.shell.layout.clone();
                let left = shell
                    .shell
                    .layout
                    .first_panel_in_zone(ZonePosition::Left)
                    .unwrap();
                let first = crate::shell::PanelTab::tool(crate::features::id::VIEW_PREVIEW);
                let second = crate::shell::PanelTab::tool(crate::features::id::VIEW_ASSISTANT);
                let source = shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Left, first.clone())
                    .unwrap();
                let extra = shell
                    .shell
                    .layout
                    .drop_tab(
                        &source,
                        &first,
                        &crate::shell::PanelDrop::Edge {
                            panel: left,
                            edge: crate::shell::PanelEdge::Right,
                        },
                        (800.0, 600.0),
                        std::time::Instant::now(),
                    )
                    .unwrap();
                let source = shell
                    .shell
                    .layout
                    .open_tab(ZonePosition::Left, second.clone())
                    .unwrap();
                shell
                    .shell
                    .layout
                    .drop_tab(
                        &source,
                        &second,
                        &crate::shell::PanelDrop::Edge {
                            panel: extra,
                            edge: crate::shell::PanelEdge::Right,
                        },
                        (800.0, 600.0),
                        std::time::Instant::now(),
                    )
                    .unwrap();
                original
            });
            let saved = shell.read(cx).shell.layout.capture();
            (original, document, saved, window.viewport_size())
        })
        .unwrap();
    for requested in [gpui::size(gpui::px(480.0), gpui::px(320.0)), size] {
        handle
            .update(cx, |_, window, _| window.resize(requested))
            .unwrap();
        gpui::Timer::after(std::time::Duration::from_millis(150)).await;
        handle
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
                root.update(cx, |root, cx| {
                    assert_eq!(root.editor.read(cx).document_id(), document);
                    let shell = root.shell.read(cx);
                    assert_eq!(shell.shell.layout.capture(), saved);
                    let viewport = window.viewport_size();
                    for bounds in shell.panel_bounds.borrow().values() {
                        assert!(
                            bounds.origin.x >= gpui::px(0.0) && bounds.origin.y >= gpui::px(0.0)
                        );
                        assert!(
                            bounds.origin.x + bounds.size.width <= viewport.width + gpui::px(1.0)
                        );
                        assert!(
                            bounds.origin.y + bounds.size.height <= viewport.height + gpui::px(1.0)
                        );
                    }
                })
                .unwrap();
            })
            .unwrap();
    }
    root.update(cx, |root, cx| {
        root.shell.update(cx, |shell, cx| {
            shell.shell.layout = original;
            cx.notify();
        });
    })
    .unwrap();
    eprintln!("SMOKE Panel layout resize preserves layout and document OK");
}

#[cfg(target_os = "windows")]
async fn simulate_tab_drag(
    root: &WeakEntity<DesktopRoot>,
    shell: &Entity<Shell>,
    handle: gpui::AnyWindowHandle,
    cancel: bool,
    destination: Option<(gpui::AnyWindowHandle, &Entity<Shell>)>,
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
            let bounds = shell.read(cx).zone_panel_bounds.borrow()[&ZonePosition::Main];
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
    // smoke モードの自プロセス Window のみにイベントを送る。
    unsafe { PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(1), start) }.unwrap();
    let moved = LPARAM(start.0 + 20);
    unsafe { PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(1), moved) }.unwrap();
    wait_for_drag_state(cx, true, original_window_count + 1).await;
    // MouseDownによる文書選択のworker応答を待つ。未完了操作中は製品側が移動を拒否する。
    wait_for(root, cx, None).await;
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
        wait_for_drag_state(cx, false, original_window_count).await;
    }
    // sourceの選択によるactivateが終わった後でdrop先を最前面にする。
    // 先に上げるとRootのactivateが覆い、別Windowへのdropを検証できない。
    if let Some((target, shell)) = destination {
        let destination = show_drop_target(target, shell, cx);
        let mut point = windows::Win32::Foundation::POINT {
            x: destination.x,
            y: destination.y,
        };
        assert!(unsafe { ScreenToClient(hwnd, &mut point) }.as_bool());
        end =
            LPARAM(((point.y as i16 as u16 as u32) << 16 | point.x as i16 as u16 as u32) as isize);
    }
    unsafe { PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), end) }.unwrap();
    wait_for_drag_state(
        cx,
        false,
        original_window_count + usize::from(!cancel && destination.is_none()),
    )
    .await;
}

#[cfg(target_os = "windows")]
async fn verify_panel_drag_resize(
    root: &WeakEntity<DesktopRoot>,
    handle: gpui::AnyWindowHandle,
    cx: &mut gpui::AsyncApp,
) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE},
    };
    let shell = root.update(cx, |root, _| root.shell.clone()).unwrap();
    let original = shell
        .read_with(cx, |shell, _| shell.shell.layout.clone())
        .unwrap();
    shell
        .update(cx, |shell, cx| {
            shell
                .shell
                .layout
                .open_tab(
                    ZonePosition::Left,
                    PanelTab::tool(crate::features::id::VIEW_PREVIEW),
                )
                .unwrap();
            cx.notify();
        })
        .unwrap();
    let _ = show_drop_target(handle, &shell, cx);
    let count = cx.update(|cx| cx.windows().len()).unwrap();
    let (hwnd, start, moved, end) = handle
        .update(cx, |_, window, cx| {
            window.draw(cx).clear();
            let shell = shell.read(cx);
            let source = shell.zone_panel_bounds.borrow()[&ZonePosition::Left];
            let target = shell.zone_panel_bounds.borrow()[&ZonePosition::Left];
            let scale = window.scale_factor();
            let pack = |x: f32, y: f32| {
                LPARAM(
                    (((y * scale).round() as i16 as u16 as u32) << 16
                        | (x * scale).round() as i16 as u16 as u32) as isize,
                )
            };
            let x = f32::from(source.origin.x) + 50.0;
            let y = f32::from(source.origin.y) + 17.0;
            let end_x = f32::from(target.origin.x + target.size.width) - 2.0;
            let end_y = f32::from(target.origin.y + target.size.height / 2.0);
            let RawWindowHandle::Win32(raw) =
                HasWindowHandle::window_handle(window).unwrap().as_raw()
            else {
                panic!("Windows handle required")
            };
            (
                raw.hwnd.get(),
                pack(x, y),
                pack(x + 20.0, y),
                pack(end_x, end_y),
            )
        })
        .unwrap();
    let hwnd = HWND(hwnd as *mut _);
    unsafe { PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(1), start) }.unwrap();
    unsafe { PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(1), moved) }.unwrap();
    wait_for_drag_state(cx, true, count + 1).await;
    unsafe { PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(1), end) }.unwrap();
    gpui::Timer::after(std::time::Duration::from_millis(80)).await;
    unsafe { PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), end) }.unwrap();
    wait_for_drag_state(cx, false, count).await;
    gpui::Timer::after(std::time::Duration::from_millis(160)).await;
    let (before, down, shifted) = handle
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
            let shell = shell.read(cx);
            let files = PanelTab::tool(crate::features::id::VIEW_FILES);
            let added = shell
                .shell
                .layout
                .panel_for_tab(&files)
                .expect("moved file tab");
            assert_eq!(
                shell.shell.layout.panel(&added).unwrap().position,
                ZonePosition::Left
            );
            let ids = shell
                .shell
                .layout
                .panel_ids()
                .into_iter()
                .filter(|id| {
                    shell
                        .shell
                        .layout
                        .panel(id)
                        .is_some_and(|panel| panel.position == ZonePosition::Left)
                })
                .collect::<Vec<_>>();
            assert_eq!(ids.len(), 2);
            let first = shell.panel_bounds.borrow()[&ids[0]];
            let scale = window.scale_factor();
            let x = f32::from(first.origin.x + first.size.width) + 4.0;
            let y = f32::from(first.origin.y + first.size.height / 2.0);
            let pack = |x: f32, y: f32| {
                LPARAM(
                    (((y * scale).round() as i16 as u16 as u32) << 16
                        | (x * scale).round() as i16 as u16 as u32) as isize,
                )
            };
            (shell.shell.layout.capture(), pack(x, y), pack(x - 40.0, y))
        })
        .unwrap();
    unsafe { PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(1), down) }.unwrap();
    unsafe { PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(1), shifted) }.unwrap();
    unsafe { PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), shifted) }.unwrap();
    gpui::Timer::after(std::time::Duration::from_millis(100)).await;
    shell
        .update(cx, |shell, cx| {
            assert_ne!(
                shell.shell.layout.capture(),
                before,
                "Panel boundary resize must change layout"
            );
            shell.shell.layout = original;
            cx.notify();
        })
        .unwrap();
    eprintln!("SMOKE Panel edge drop and boundary resize OK");
}

#[cfg(target_os = "windows")]
async fn wait_for_drag_state(cx: &mut gpui::AsyncApp, dragging: bool, window_count: usize) {
    for _ in 0..125 {
        if cx
            .update(|cx| cx.has_active_drag() == dragging && cx.windows().len() == window_count)
            .unwrap()
        {
            return;
        }
        gpui::Timer::after(std::time::Duration::from_millis(16)).await;
    }
    panic!("Native drag state did not settle (dragging={dragging}, windows={window_count})");
}

#[cfg(target_os = "windows")]
fn show_drop_target(
    handle: gpui::AnyWindowHandle,
    shell: &Entity<Shell>,
    cx: &mut gpui::AsyncApp,
) -> drag::ScreenPoint {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, POINT},
        UI::WindowsAndMessaging::{
            GetClassNameW, HWND_TOP, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            SetWindowPos, ShowWindow, WindowFromPoint,
        },
    };
    handle
        .update(cx, |_, window, cx| {
            window.draw(cx).clear();
            let shell = shell.read(cx);
            let position = shell
                .zone_panel_bounds
                .borrow()
                .get(&ZonePosition::Main)
                .copied()
                .or_else(|| shell.zone_bounds.borrow().get(&ZonePosition::Main).copied())
                .expect("Main Zone bounds")
                .center();
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
            let hit = unsafe {
                WindowFromPoint(POINT {
                    x: point.x,
                    y: point.y,
                })
            };
            let mut class = [0u16; 256];
            let length = unsafe { GetClassNameW(hit, &mut class) } as usize;
            assert!(
                drag::is_source_window(window, point),
                "Native hit test did not select the fixture window: hit class={}",
                String::from_utf16_lossy(&class[..length]),
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
    // 新しいNative Windowの生成・初回描画を待つ。
    wait_for(root, cx, Some(3)).await;
    simulate_tab_drag(
        root,
        &first_shell,
        first_handle,
        false,
        Some((second_handle, &second_shell)),
        cx,
    )
    .await;
    wait_for(root, cx, None).await;
    root.update(cx, |root, cx| {
        assert_eq!(root.group.read(cx).documents(second), vec![id.clone()]);
        assert!(root.group.read(cx).documents(first).is_empty());
    })
    .unwrap();
    simulate_tab_drag(
        root,
        &second_shell,
        second_handle,
        false,
        Some((handle, &root_shell)),
        cx,
    )
    .await;
    wait_for(root, cx, None).await;
    root.update(cx, |root, cx| {
        assert!(root.group.read(cx).documents(ROOT_HOST).contains(id));
        assert_eq!(root.editor.read(cx).document_id().as_ref(), Some(id));
    })
    .unwrap();
    simulate_tab_drag(
        root,
        &root_shell,
        handle,
        false,
        Some((second_handle, &second_shell)),
        cx,
    )
    .await;
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
