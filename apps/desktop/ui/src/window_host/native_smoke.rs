//! 隔離したWorkspaceでNative Windowの寿命・所属・共有状態を検証する。
use super::*;

pub(crate) fn run_native_smoke(
    root: &mut DesktopRoot,
    window: &mut Window,
    cx: &mut Context<DesktopRoot>,
) {
    let id = root.editor.read(cx).document_id().unwrap();
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
        handle
            .update(cx, |_, _, cx| {
                this.update(cx, |root, cx| {
                    root.detach_tab(0, PanelTab::Document(id.clone()), cx);
                    eprintln!("SMOKE detached native windows={}", cx.windows().len());
                })
            })
            .unwrap()
            .unwrap();
        wait_for(&this, cx, None).await;
        gpui::Timer::after(std::time::Duration::from_millis(500)).await;
        eprintln!("SMOKE idle sample begin");
        gpui::Timer::after(std::time::Duration::from_secs(5)).await;
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
        peer.update(cx, |other, cx| {
            other.editor.update(cx, |editor, cx| {
                editor.submit(
                    false,
                    |client| {
                        client.editor.new_document();
                        Ok(())
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
