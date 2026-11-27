use super::*;
use gpui::Context;

impl DesktopRoot {
    /// 他Clientが共有したタブ構成をUIへ反映する。
    ///
    /// 未確定の入力がある間は適用せず、Idleで再試行する。適用した値はそのまま
    /// 送り返さない。適用中に届いた次の変更は、反映完了後にもう一度適用する。
    pub(super) fn apply_remote_session_view(&mut self, cx: &mut Context<Self>) {
        if self.session_view.is_none() {
            return;
        }
        if self.session_view_applying {
            // 反映中の通知は捨てずに記録し、完了後にもう一度確認する。
            self.session_view_pending = true;
            return;
        }
        if self.editor.read(cx).has_in_flight() {
            return;
        }
        self.session_view_applying = true;
        let worker = self.worker.clone();
        let submitted = worker.submit(
            cx,
            |client| client.apply_session_view(),
            |root, result, cx| {
                root.session_view_applying = false;
                let applied = match result {
                    Ok(Some(snapshot)) => {
                        root.apply_shared_snapshot(&snapshot, cx);
                        true
                    }
                    Ok(None) => false,
                    Err(error) => {
                        eprintln!("Workspace view を反映できません: {error}");
                        false
                    }
                };
                // 反映した時と、反映中に見送った通知がある時だけ再確認する。
                // 変化が無いのに再投入すると、worker への送信が終わらない背景ループになる。
                let pending = std::mem::take(&mut root.session_view_pending);
                if applied || pending {
                    root.apply_remote_session_view(cx);
                }
            },
        );
        if submitted.is_err() {
            self.session_view_applying = false;
        }
    }

    /// 自分のタブ開閉とアクティブ文書を、短い debounce で共有へ送る。
    ///
    /// 保存の debounce とは分離し、タブ操作が続く間は最後の状態だけを送る。
    /// 再同期の要求より後にだけ送り、共有済みの状態を古い値で上書きしない。
    /// タブ構成とアクティブ文書が変わっていない編集では待機を延長しない。
    pub(crate) fn schedule_session_view_publish(&mut self, cx: &mut Context<Self>) {
        if self.session_view.is_none()
            || self.group.read(cx).closing
            || self.group.read(cx).synchronization_required
            || self.task_context_resolution == TaskContextResolution::Pending
            || self.editor.read(cx).has_in_flight()
        {
            return;
        }
        // 既に同じ構成の送信待ちがあるなら、カーソル移動や入力で待機を延ばさない。
        // 送信自体は worker 側で直前の構成と比較して省略される。
        if self.session_view_task.is_some() {
            return;
        }
        self.session_view_task = Some(cx.spawn(async move |this, cx| {
            crate::timer::Timer::after(std::time::Duration::from_millis(150)).await;
            let _ = this.update(cx, |root, cx| {
                // 見送る場合も待機印を消す。残すと以後の予約がすべて止まる。
                root.session_view_task = None;
                if root.group.read(cx).closing
                    || root.group.read(cx).synchronization_required
                    || root.task_context_resolution == TaskContextResolution::Pending
                    || root.editor.read(cx).has_in_flight()
                {
                    return;
                }
                let worker = root.worker.clone();
                let _ = worker.submit(
                    cx,
                    |client| client.publish_session_view(),
                    |_, result, _| {
                        // 失敗したら署名を確定させず、次の変更で再送させる。
                        if let Err(error) = result {
                            eprintln!("Workspace view を共有できません: {error}");
                        }
                    },
                );
            });
        }));
    }

    /// バックグラウンド復帰や再接続で snapshot を取り直し、取りこぼしを再同期する。
    ///
    /// 継続中の debounce があれば取り消し、再同期した snapshot より先に古い値を
    /// 送らないようにする。Editor の処理中は保留し、Idle で実行する。
    pub(super) fn resync_remote_session_view(&mut self, cx: &mut Context<Self>) {
        if self.session_view.is_none() || self.group.read(cx).closing {
            return;
        }
        if self.editor.read(cx).has_in_flight() {
            self.session_view_resync_pending = true;
            return;
        }
        self.session_view_resync_pending = false;
        self.session_view_task.take();
        let worker = self.worker.clone();
        let _ = worker.submit(
            cx,
            |client| client.resync_session_view(),
            |root, result, cx| match result {
                Ok(Some(snapshot)) => root.apply_shared_snapshot(&snapshot, cx),
                Ok(None) => {}
                Err(error) => eprintln!("Workspace view を再同期できません: {error}"),
            },
        );
    }

    /// 共有 view が古い可能性を表示へ反映する。状態が変わった時だけ再描画する。
    pub(super) fn sync_session_view_stale(&mut self, cx: &mut Context<Self>) {
        // Workspace を閉じた後は共有対象が無いため、古い表示の警告を出さない。
        let stale = self.shell.read(cx).session.workspace_id.is_some()
            && self
                .session_view
                .as_ref()
                .is_some_and(|handle| session_view_watch_of(handle).is_stale());
        if self.session_view_stale == stale {
            return;
        }
        self.session_view_stale = stale;
        let message = if stale {
            "共有中のタブ構成は最新ではない可能性があります"
        } else {
            ""
        };
        // 通常の status とは別枠で出す。他のメッセージを上書きしない。
        self.shell
            .update(cx, |shell, cx| shell.set_warning(message.to_owned(), cx));
    }
}
