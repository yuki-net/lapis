mod appearance;
mod requests;
mod sessions;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use desktop_client::{ResolvedSettings, RestoredTerminal, TerminalSession};
use editor_core::ConversationId;
use terminal::{TerminalError, TerminalInput};

use appearance::{terminal_caret, terminal_font_family, terminal_number, terminal_profile};
use requests::{AutoStart, TerminalRequests};
use sessions::TerminalSessions;

/// Terminal Resourceの変化。Shellは受信してタブをResource参照へ更新する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalFeatureEvent {
    /// 新しい Terminal を開始した。
    Started(terminal::TerminalId),
    /// 新しいタブを開く操作。タブは Shell が1つ増やす。
    NewTabRequested,
}

pub(crate) struct TerminalFeature {
    worker: crate::client_adapter::ClientWorker<TerminalSessions>,
    live_sessions: Arc<AtomicBool>,
    extension_settings: Option<desktop_client::ExtensionContributionSession>,
    pub(super) terminals: Vec<terminal::TerminalSnapshot>,
    /// 各 Terminal の操作権。表示に使う。
    pub(super) owners: HashMap<String, Option<String>>,
    /// `terminals` に適用した最後の版。古い結果で上書きしないために使う。
    projected: u64,
    pub(super) discovered: Vec<terminal::TerminalSnapshot>,
    pub(super) selected: Option<terminal::TerminalId>,
    selected_by_context: HashMap<(Option<PathBuf>, ConversationId), terminal::TerminalId>,
    bound_resource: Option<terminal::TerminalId>,
    pub(super) focus: gpui::FocusHandle,
    pub(super) marked: String,
    /// 端末ごとの送信待ち入力。Text のバッチ化と入力順序を担う。
    pub(super) inputs: Vec<(terminal::TerminalId, super::input::TerminalInputQueue)>,
    input_sequence: u64,
    input_flush_scheduled: bool,
    /// 失敗後に再同期付きの再送を 1 回だけ行うための印。
    input_recovery: bool,
    resyncing_inputs: bool,
    /// 再同期を開始したコンテキスト世代。切り替え後の古い結果を捨てるために使う。
    input_epoch: u64,
    /// 入力失敗で表示中のエラー。次の入力成功時に同じ端末の同じ message だけを消す。
    input_error: Option<(terminal::TerminalId, String)>,
    /// 右クリック／セッション一覧ボタンで開くメニューのアンカー。None のときは閉じている。
    pub(super) context_menu: Option<()>,
    pub(super) context_anchor: Option<gpui::Point<gpui::Pixels>>,
    /// 描画に使うフォント設定。設定 UI から変更できるようにする。
    pub(super) font_family: gpui::SharedString,
    pub(super) font_size: f32,
    pub(super) line_height_percent: u8,
    pub(super) caret_shape: settings::TerminalCaretShape,
    /// Host が許可する Terminal profile。既定プロファイルの解決と例外操作に使う。
    pub(super) profiles: terminal::TerminalProfiles,
    /// 設定画面へ公開する profile 一覧。設定の供給元と共有する。
    pub(super) profiles_catalog: std::rc::Rc<std::cell::RefCell<super::TerminalProfileCatalog>>,
    /// 設定で選ばれた既定 profile ID。`None` は Host の既定に従う。
    pub(super) default_profile: Option<terminal::TerminalProfileId>,
    /// ターミナルタブを初めて開いたときの自動起動状態。
    auto_start: AutoStart,
    /// タブ表示後に自動起動を要求されたか。
    pending_auto_start: bool,
    /// ターミナルタブが画面に出ているか。表示の切り替えだけでは終了しない。
    tab_visible: bool,
    /// Host の profile 一覧を読み終えたか。読み終えるまで既定 profile は決めない。
    profiles_loaded: bool,
    /// 非同期 backend が一時的に一覧を返せなかった場合、次の変更通知で再取得する。
    profiles_retry: bool,
    pub(super) last_metrics: Option<super::view::TerminalMetrics>,
    pub(super) last_size: Option<(terminal::TerminalId, (u16, u16))>,
    /// 行の内容から shaped line を引き当てるキャッシュ。フォント変更時に捨てる。
    pub(super) line_cache: super::canvas::TerminalLineCache,
    /// 出力の notify を frame 単位に集約するための印。render の冒頭で戻す。
    pub(super) pending_output_render: bool,
    /// Terminal出力の表示状態。
    /// None = 未処理（output変更を検知していない）
    /// Some(None) = scroll pending（outputは変更済みだがscroll-followが未完了）
    /// Some(Some((id, revision))) = 完了（scroll-followが成功した）
    pub(super) visible_output: Option<Option<(terminal::TerminalId, u64)>>,
    pub(super) root: Option<std::path::PathBuf>,
    conversation: ConversationId,
    requests: TerminalRequests,
    pub(super) message: String,
    message_from_poll: bool,
    pub(super) scroll: crate::components::ScrollState,
}

impl gpui::EventEmitter<TerminalFeatureEvent> for TerminalFeature {}
impl TerminalFeature {
    /// より新しい版だけを画面へ適用する。古い action / poll の結果で上書きしない。
    /// 端末一覧と操作権を同じ版で投影する。古い版なら両方とも捨てる。
    fn apply_projection(
        &mut self,
        version: u64,
        terminals: Vec<terminal::TerminalSnapshot>,
        owners: HashMap<String, Option<String>>,
    ) {
        if version < self.projected {
            return;
        }
        self.projected = version;
        self.terminals = terminals;
        self.owners = owners;
    }

    /// 設定の変更を描画へ反映する。フォントや行送りが変わると列数・行数も変わる。
    fn apply_terminal_appearance(
        &mut self,
        values: &ResolvedSettings,
        cx: &mut gpui::Context<Self>,
    ) {
        let family: gpui::SharedString = terminal_font_family(values)
            .unwrap_or_else(|| super::default_font_family().to_owned())
            .into();
        let size = terminal_number(
            values,
            "font-size",
            desktop_client::ResolvedSettings::terminal_font_size,
        )
        .unwrap_or_else(super::default_font_size) as f32;
        let line_height_percent = terminal_number(
            values,
            "line-height-percent",
            desktop_client::ResolvedSettings::terminal_line_height_percent,
        )
        .unwrap_or_else(super::default_line_height_percent);
        let caret_shape = terminal_caret(values).unwrap_or_default();
        let default_profile = terminal_profile(values).map(terminal::TerminalProfileId::new);
        if self.font_family == family
            && self.font_size == size
            && self.line_height_percent == line_height_percent
            && self.caret_shape == caret_shape
            && self.default_profile == default_profile
        {
            return;
        }
        self.font_family = family;
        self.font_size = size;
        self.line_height_percent = line_height_percent;
        self.caret_shape = caret_shape;
        self.default_profile = default_profile;
        self.last_metrics = None;
        self.last_size = None;
        self.line_cache.clear();
        cx.notify();
    }

    pub(crate) fn refresh_extension_settings(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(values) = self
            .extension_settings
            .as_ref()
            .and_then(|session| session.resolved())
        {
            self.apply_terminal_appearance(&values, cx);
        }
    }

    /// 出力による再描画を frame 単位に集約する。
    ///
    /// 同じ frame に複数の delta が届いても notify は 1 回にし、render の冒頭で
    /// 印を戻す。render 後に届いた delta は次の frame で描画される。
    fn request_output_render(&mut self, cx: &mut gpui::Context<Self>) {
        if self.pending_output_render {
            return;
        }
        self.pending_output_render = true;
        cx.notify();
    }

    pub(crate) fn summaries(&self) -> Vec<terminal::TerminalSnapshot> {
        self.terminals.clone()
    }
    pub(crate) fn persistence_summaries(&self) -> Vec<RestoredTerminal> {
        self.terminals
            .iter()
            .map(|terminal| RestoredTerminal {
                id: Some(terminal.id.clone()),
                cwd: terminal.cwd.clone(),
                status: terminal.status,
                columns: terminal.screen.size.columns,
                rows: terminal.screen.size.rows,
            })
            .collect()
    }
    pub(crate) fn persistence_owner(&self) -> Option<ConversationId> {
        (self.requests.pending_actions == 0 && !self.requests.context_unavailable)
            .then(|| self.conversation.clone())
    }
    /// テスト用: 指定 Task と root に一致し、参照先が実在する場合だけ保存先を返す。
    #[cfg(test)]
    pub(crate) fn persistence_owner_for(
        &self,
        task: &ConversationId,
        root: Option<&std::path::Path>,
    ) -> Option<ConversationId> {
        self.persistence_owner().filter(|owner| {
            owner == task
                && self.root.as_deref() == root
                && self
                    .bound_resource
                    .as_ref()
                    .is_none_or(|id| self.terminals.iter().any(|terminal| &terminal.id == id))
        })
    }

    /// タブの参照先 Resource を固定する。復元時と開始時に使う。
    #[cfg(test)]
    pub(crate) fn bind_resource(&mut self, resource: Option<terminal::TerminalId>) {
        self.bound_resource = resource;
    }

    /// タブの参照先が無くなったとき、固定参照とそれに基づく自動起動の判断を捨てる。
    ///
    /// 固定参照が残っていると `ensure_auto_start` が「既存 Terminal への再アタッチ」と
    /// みなして起動を拒否するため、参照先が無いと確定した時点で解除する。
    /// 参照が元から無い場合は状態を変えない。
    #[cfg(test)]
    pub(crate) fn release_resource(&mut self) {
        if self.bound_resource.take().is_some() {
            // 参照先を前提にした判定を取り消し、表示中のタブで改めて判定できるようにする。
            self.auto_start = AutoStart::Idle;
            self.pending_auto_start = self.tab_visible;
        }
    }

    /// タブが参照するHost Terminalへ接続する。見つからない場合も新しいshellは開始しない。
    pub(crate) fn show_resource(
        &mut self,
        resource: terminal::TerminalId,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.bound_resource.as_ref() == Some(&resource) {
            return;
        }
        self.bound_resource = Some(resource.clone());
        self.attach(resource, cx);
    }
    /// タブ1つ分の実体を作る。
    ///
    /// タブは1セッションを持つ。Host の変更通知は機能側の Lifecycle が1本だけ
    /// 購読して全タブへ配るため、ここでは購読しない。
    pub(crate) fn new(
        session: TerminalSession,
        root: Option<PathBuf>,
        conversation: ConversationId,
        settings_catalog: std::rc::Rc<std::cell::RefCell<super::TerminalProfileCatalog>>,
        extension_settings: Option<desktop_client::ExtensionContributionSession>,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let terminals = session.terminals().to_vec();
        let coordinator =
            crate::features::settings_coordinator::SettingsCoordinator::try_shared(cx);
        let values = extension_settings
            .as_ref()
            .and_then(|session| session.resolved())
            .or_else(|| {
                coordinator
                    .as_ref()
                    .map(|coordinator| coordinator.read(cx).values.clone())
            });
        let font_family: gpui::SharedString = values
            .as_ref()
            .and_then(terminal_font_family)
            .unwrap_or_else(|| super::default_font_family().to_owned())
            .into();
        let font_size = values
            .as_ref()
            .and_then(|values| {
                terminal_number(
                    values,
                    "font-size",
                    desktop_client::ResolvedSettings::terminal_font_size,
                )
            })
            .unwrap_or_else(super::default_font_size) as f32;
        let line_height_percent = values
            .as_ref()
            .and_then(|values| {
                terminal_number(
                    values,
                    "line-height-percent",
                    desktop_client::ResolvedSettings::terminal_line_height_percent,
                )
            })
            .unwrap_or_else(super::default_line_height_percent);
        let caret_shape = values.as_ref().and_then(terminal_caret).unwrap_or_default();
        let default_profile = values
            .as_ref()
            .and_then(terminal_profile)
            .map(terminal::TerminalProfileId::new);
        let sessions = TerminalSessions::new(session, root.clone(), conversation.clone());
        let live_sessions = Arc::clone(&sessions.live_sessions);
        let mut feature = Self {
            worker: crate::client_adapter::ClientWorker::new(sessions),
            live_sessions,
            extension_settings,
            terminals,
            owners: HashMap::new(),
            projected: 0,
            discovered: Vec::new(),
            selected: None,
            selected_by_context: HashMap::new(),
            bound_resource: None,
            focus: cx.focus_handle(),
            marked: String::new(),
            inputs: Vec::new(),
            input_sequence: 0,
            input_flush_scheduled: false,
            input_recovery: false,
            resyncing_inputs: false,
            input_epoch: 0,
            input_error: None,
            context_menu: None,
            context_anchor: None,
            font_family,
            font_size,
            line_height_percent,
            caret_shape,
            profiles: terminal::TerminalProfiles::default(),
            profiles_catalog: settings_catalog,
            default_profile,
            auto_start: AutoStart::Idle,
            pending_auto_start: false,
            tab_visible: false,
            profiles_loaded: false,
            profiles_retry: false,
            last_metrics: None,
            last_size: None,
            line_cache: Default::default(),
            pending_output_render: false,
            visible_output: None, // None = 未処理
            root,
            conversation,
            requests: TerminalRequests::default(),
            message: String::new(),
            message_from_poll: false,
            scroll: crate::components::ScrollState::new(),
        };
        if let Some(coordinator) = coordinator {
            cx.observe(&coordinator, |feature, coordinator, cx| {
                let values = feature
                    .extension_settings
                    .as_ref()
                    .and_then(|session| session.resolved())
                    .unwrap_or_else(|| coordinator.read(cx).values.clone());
                feature.apply_terminal_appearance(&values, cx);
            })
            .detach();
        }
        feature.load_profiles(cx);
        feature
    }

    /// テスト用: profile の供給元を共有しない実体を作る。
    #[cfg(test)]
    pub(crate) fn test_new(
        session: TerminalSession,
        root: Option<PathBuf>,
        conversation: ConversationId,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        Self::new(
            session,
            root,
            conversation,
            std::rc::Rc::new(std::cell::RefCell::new(
                super::TerminalProfileCatalog::default(),
            )),
            None,
            cx,
        )
    }

    /// Host が許可する profile を worker 経由で取得する。UI thread を止めない。
    fn load_profiles(&mut self, cx: &mut gpui::Context<Self>) {
        self.profiles_loaded = false;
        let Some(root) = self.root.clone() else {
            self.profiles = terminal::TerminalProfiles::default();
            self.profiles_retry = false;
            super::publish_profile_catalog(&self.profiles, &self.profiles_catalog, cx);
            self.profiles_loaded = true;
            return;
        };
        let load_root = root.clone();
        let _ = self.worker.submit(
            cx,
            move |sessions| Ok(sessions.session.profiles(&load_root)),
            move |feature, result, cx| {
                if feature.root.as_deref() != Some(root.as_path()) {
                    return;
                }
                match result {
                    Ok(Ok(profiles)) => {
                        feature.profiles = profiles;
                        feature.profiles_retry = false;
                        feature.profiles_loaded = true;
                    }
                    Ok(Err(_)) | Err(_) => {
                        feature.profiles = terminal::TerminalProfiles::default();
                        feature.profiles_retry = true;
                        feature.profiles_loaded = false;
                    }
                }
                super::publish_profile_catalog(&feature.profiles, &feature.profiles_catalog, cx);
                // profile が揃ってから既定 profile を解決する。
                feature.ensure_auto_start(cx);
                cx.notify();
            },
        );
    }

    pub(crate) fn set_context(
        &mut self,
        root: Option<std::path::PathBuf>,
        conversation: ConversationId,
        summaries: Vec<RestoredTerminal>,
        cx: &mut gpui::Context<Self>,
    ) {
        if !self.requests.context_unavailable
            && self.conversation == conversation
            && self.root == root
        {
            return;
        }
        let next = conversation.clone();
        let next_root = root.clone();
        self.requests.invalidate();
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                let terminals = sessions.switch_context(next_root, next, &summaries);
                Ok((sessions.version(), terminals, sessions.owners().clone()))
            },
            move |feature, result, cx| {
                if !feature.requests.finish_action(token) {
                    return;
                }
                match result {
                    Ok((version, terminals, owners)) => {
                        feature.apply_projection(version, terminals, owners);
                    }
                    Err(error) => {
                        feature.requests.context_unavailable = true;
                        feature.root = None;
                        feature.terminals.clear();
                        feature.message = error;
                        feature.message_from_poll = false;
                    }
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.requests.context_unavailable = true;
            self.root = None;
            self.terminals.clear();
            self.message = error;
            self.message_from_poll = false;
            cx.notify();
            return;
        }
        self.requests.context_unavailable = false;
        if let Some(selected) = self.active_id() {
            self.selected_by_context
                .insert((self.root.clone(), self.conversation.clone()), selected);
        }
        self.selected = self
            .selected_by_context
            .get(&(root.clone(), conversation.clone()))
            .cloned();
        self.discovered.clear();
        self.marked.clear();
        // 送信先の session が変わるため、未送信入力は引き継がない。
        self.inputs.clear();
        self.input_flush_scheduled = false;
        self.input_recovery = false;
        self.resyncing_inputs = false;
        // 進行中の再同期の結果を、切り替え後の状態へ適用しない。
        self.input_epoch = self.input_epoch.wrapping_add(1);
        let input_error = self.input_error.take();
        if input_error.is_some_and(|(_, error)| error == self.message) {
            self.message.clear();
        }
        self.root = root;
        self.conversation = conversation;
        self.terminals.clear();
        // コンテキストが変われば、次にタブが現れたとき改めて自動起動する。
        // タブが見えたままなら、profile 取得後に改めて 1 回だけ起動する。
        self.auto_start = AutoStart::Idle;
        self.pending_auto_start = self.tab_visible;
        self.load_profiles(cx);
        // 固定参照を持つタブは、切り替え先のコンテキストで同じ PTY へ戻す。
        // 戻さないと表示だけが空のまま残る。
        if let Some(resource) = self.bound_resource.clone() {
            self.attach(resource, cx);
        }
        cx.notify();
    }
    /// 新しいシェルは新しいタブとして開く。タブは1セッションだけを持つ。
    pub(super) fn request_new_tab(&mut self, cx: &mut gpui::Context<Self>) {
        cx.emit(TerminalFeatureEvent::NewTabRequested);
    }

    /// Host の変更通知を受けたときに、この実体の表示を更新する。
    /// Shell が1本の通知を全タブへ配るため、複数タブでも購読は1回で済む。
    pub(crate) fn note_change(&mut self, cx: &mut gpui::Context<Self>) {
        if self.profiles_retry {
            self.profiles_retry = false;
            self.load_profiles(cx);
        }
        self.poll(cx);
    }

    /// ターミナルタブが見えているかを Feature へ伝える。見えたら既定 profile で1回だけ自動起動する。
    ///
    /// パネルや Zone を閉じただけ、または他 Task の表示へ切り替えただけでは
    /// セッションを終了しない（Workspace の detach と同じ扱い）。
    /// 終了はタブを閉じる操作 `close_session` だけが行う。
    pub(crate) fn sync_tab_presence(&mut self, visible: bool, cx: &mut gpui::Context<Self>) {
        self.tab_visible = visible;
        self.pending_auto_start = visible;
        // 表示中は、コンテキスト切替などで未起動に戻ったときも1回だけ起動を試みる。
        if visible {
            self.ensure_auto_start(cx);
        }
    }

    /// profile とコンテキストが揃った時点で、保留中の自動起動を1回だけ実行する。
    fn ensure_auto_start(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.pending_auto_start || !self.profiles_loaded || self.root.is_none() {
            return;
        }
        self.pending_auto_start = false;
        if self.auto_start != AutoStart::Idle {
            return;
        }
        if self.bound_resource.is_some() {
            // 既存の Host Terminal を参照するタブ。再アタッチに任せ、新しい shell は起動しない。
            self.auto_start = AutoStart::Finished;
            return;
        }
        if !self.terminals.is_empty() {
            // 復元した既存セッションがある。新しく shell は起動しない。
            self.auto_start = AutoStart::Finished;
            return;
        }
        self.auto_start = AutoStart::Finished;
        let profile = self.resolved_default_profile();
        self.start_profile(profile, cx);
    }

    /// ターミナルタブを閉じたときの終了。表示中のコンテキストの Session を止める。
    pub(crate) fn close_session(&mut self, cx: &mut gpui::Context<Self>) {
        // 閉じた直後に開き直しても、次の表示で自動起動できるようにする。
        self.auto_start = AutoStart::Idle;
        self.pending_auto_start = false;
        self.tab_visible = false;
        self.requests.invalidate();
        if self.root.is_none() {
            self.reset_after_close();
            cx.notify();
            return;
        }
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            |sessions| {
                let (outcome, terminals) = sessions.close_all();
                Ok((
                    outcome,
                    sessions.version(),
                    terminals,
                    sessions.owners().clone(),
                ))
            },
            move |feature, result, cx| {
                if !feature.requests.finish_action(token) {
                    return;
                }
                match result {
                    Ok((outcome, version, terminals, owners)) => {
                        feature.apply_projection(version, terminals, owners);
                        feature.message = outcome.err().unwrap_or_default();
                    }
                    Err(error) => feature.message = error,
                }
                feature.message_from_poll = false;
                // 閉じる応答より前にタブを開き直した場合も、表示中のタブを空のまま残さない。
                // reset_after_close は固定参照も消すため、Terminal が残っていなければ
                // 改めて既定 profile の自動起動を行う。
                feature.reset_after_close();
                feature.pending_auto_start = feature.tab_visible;
                feature.load_profiles(cx);
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            self.message_from_poll = false;
            self.reset_after_close();
            cx.notify();
        }
    }

    pub(crate) fn requires_close_confirmation(&self) -> bool {
        self.live_sessions.load(Ordering::Acquire)
            || self.requests.pending_actions > 0
            || self.requests.context_unavailable
            || (self.pending_auto_start && self.root.is_some())
            || (self.bound_resource.is_some() && self.terminals.is_empty())
            || self.terminals.iter().any(|terminal| {
                matches!(
                    terminal.status,
                    terminal::TerminalStatus::Running | terminal::TerminalStatus::Disconnected
                )
            })
    }

    /// 設定で選ばれた既定 profile。Host が公開していない ID は Host 既定へ倒す。
    pub(super) fn resolved_default_profile(&self) -> Option<terminal::TerminalProfileId> {
        let id = self.default_profile.clone()?;
        self.profiles.find(&id).map(|_| id)
    }

    /// 閉じたタブの表示状態を捨て、次に開いたとき自動起動できるようにする。
    fn reset_after_close(&mut self) {
        self.selected = None;
        self.bound_resource = None;
        self.selected_by_context.clear();
        self.discovered.clear();
        self.inputs.clear();
        self.input_flush_scheduled = false;
        self.resyncing_inputs = false;
        self.input_error = None;
        self.visible_output = None;
        self.last_size = None;
        self.auto_start = AutoStart::Idle;
    }

    pub(super) fn start_profile(
        &mut self,
        profile: Option<terminal::TerminalProfileId>,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                let (result, terminals) = sessions
                    .action(|session| session.start_profile(&root, 120, 30, profile).map(|_| ()));
                Ok((
                    result,
                    sessions.version(),
                    terminals,
                    sessions.owners().clone(),
                ))
            },
            move |feature, result, cx| {
                if !feature.requests.finish_action(token) {
                    return;
                }
                match result {
                    Ok((result, version, terminals, owners)) => {
                        feature.apply_projection(version, terminals, owners);
                        feature.selected =
                            feature.terminals.last().map(|terminal| terminal.id.clone());
                        if result.is_ok()
                            && let Some(id) = feature.selected.clone()
                        {
                            cx.emit(TerminalFeatureEvent::Started(id));
                        }
                        feature.message = result.err().unwrap_or_default();
                        feature.message_from_poll = false;
                    }
                    Err(error) => {
                        feature.message = error;
                        feature.message_from_poll = false;
                    }
                }
                // 起動を試みた後は、タブを閉じるかコンテキストが変わるまで自動起動を繰り返さない。
                feature.auto_start = AutoStart::Finished;
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            self.message_from_poll = false;
            self.auto_start = AutoStart::Finished;
            cx.notify();
        }
    }
    pub(super) fn open_menu(
        &mut self,
        anchor: Option<gpui::Point<gpui::Pixels>>,
        cx: &mut gpui::Context<Self>,
    ) {
        self.context_anchor = anchor;
        self.context_menu = anchor.map(|_| ());
        cx.notify();
    }

    pub(super) fn send_clipboard(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.send_input(TerminalInput::Paste(text), cx);
    }

    pub(super) fn active_id(&self) -> Option<terminal::TerminalId> {
        if let Some(id) = &self.bound_resource {
            return self
                .terminals
                .iter()
                .find(|terminal| &terminal.id == id)
                .map(|terminal| terminal.id.clone());
        }
        self.selected
            .as_ref()
            .filter(|id| self.terminals.iter().any(|terminal| terminal.id == **id))
            .cloned()
            .or_else(|| self.terminals.last().map(|terminal| terminal.id.clone()))
    }

    pub(super) fn active_screen(&self) -> Option<&terminal::TerminalScreen> {
        let active = self.active_id()?;
        self.terminals
            .iter()
            .find(|terminal| terminal.id == active)
            .map(|terminal| &terminal.screen)
    }

    pub(super) fn discover(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                sessions
                    .session
                    .discover(&root)
                    .map_err(|error| error.to_string())
            },
            move |feature, result, cx| {
                if feature.requests.finish_action(token) {
                    match result {
                        Ok(terminals) => {
                            feature.discovered = terminals;
                            feature.message.clear();
                        }
                        Err(error) => feature.message = error,
                    }
                    cx.notify();
                }
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            cx.notify();
        }
    }

    pub(super) fn attach(&mut self, id: terminal::TerminalId, cx: &mut gpui::Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let selected = id.clone();
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                let (outcome, terminals) = sessions.action(|session| session.attach(&root, &id));
                Ok((
                    outcome,
                    sessions.version(),
                    terminals,
                    sessions.owners().clone(),
                ))
            },
            move |feature, result, cx| {
                if feature.requests.finish_action(token) {
                    match result {
                        Ok((outcome, version, terminals, owners)) => {
                            if outcome.is_ok() {
                                feature.selected = Some(selected);
                            }
                            feature.apply_projection(version, terminals, owners);
                            feature.message = outcome.err().unwrap_or_default();
                        }
                        Err(error) => feature.message = error,
                    }
                    cx.notify();
                }
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            cx.notify();
        }
    }

    pub(super) fn send_text(&mut self, text: String, cx: &mut gpui::Context<Self>) {
        self.send_input(TerminalInput::Text(text), cx);
    }

    pub(super) fn send_input(&mut self, input: TerminalInput, cx: &mut gpui::Context<Self>) {
        let Some(id) = self.active_id() else { return };
        match input {
            TerminalInput::Text(text) => {
                self.queue_mut(&id).enqueue_text(text);
                self.schedule_input_flush(cx);
            }
            input @ (TerminalInput::Key(_) | TerminalInput::Paste(_)) => {
                self.queue_mut(&id).enqueue_immediate(input);
                // Key/Paste は待ち Text を同じ batch に含めて即時送る。
                self.flush_terminal_input(id, cx);
            }
        }
    }

    fn queue_mut(&mut self, id: &terminal::TerminalId) -> &mut super::input::TerminalInputQueue {
        let index = match self.inputs.iter().position(|(terminal, _)| terminal == id) {
            Some(index) => index,
            None => {
                self.inputs.push((id.clone(), Default::default()));
                self.inputs.len() - 1
            }
        };
        &mut self.inputs[index].1
    }

    /// 待ち Text を 1 frame 程度まとめて送る。
    fn schedule_input_flush(&mut self, cx: &mut gpui::Context<Self>) {
        if self.input_flush_scheduled {
            return;
        }
        self.input_flush_scheduled = true;
        cx.spawn(async move |this, cx| {
            crate::timer::Timer::after(super::input::INPUT_BATCH_DELAY).await;
            this.update(cx, |feature, cx| {
                feature.input_flush_scheduled = false;
                feature.flush_pending_inputs(cx);
            })
            .ok();
        })
        .detach();
    }

    fn flush_pending_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        let ids = self
            .inputs
            .iter()
            .filter(|(_, queue)| queue.has_pending() && !queue.has_in_flight())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.flush_terminal_input(id, cx);
        }
    }

    fn flush_terminal_input(&mut self, id: terminal::TerminalId, cx: &mut gpui::Context<Self>) {
        let sequence = self.input_sequence;
        let Some(batch) = self
            .inputs
            .iter_mut()
            .find(|(terminal, _)| terminal == &id)
            .and_then(|(_, queue)| queue.begin_flush(sequence))
        else {
            return;
        };
        self.input_sequence = batch
            .sequence
            .checked_add(1)
            .expect("terminal input sequence overflow");
        let input_count = batch.inputs.len();
        let inputs = batch.inputs;
        let terminal = id.clone();
        let completed = id.clone();
        let admitted = self.worker.submit(
            cx,
            move |sessions| Ok(sessions.session.input_batch(&terminal, &inputs)),
            move |feature, result, cx| {
                let (applied, error) = match result {
                    Ok(Ok(())) => (input_count, None),
                    Ok(Err(failure)) => (failure.applied, Some(failure.message)),
                    Err(error) => (0, Some(error)),
                };
                feature.finish_input_batch(&completed, sequence, applied, error, cx);
            },
        );
        if let Err(error) = admitted {
            self.finish_input_batch(&id, sequence, 0, Some(error), cx);
        }
    }

    fn finish_input_batch(
        &mut self,
        id: &terminal::TerminalId,
        sequence: u64,
        applied: usize,
        error: Option<String>,
        cx: &mut gpui::Context<Self>,
    ) {
        let outcome = {
            let Some((_, queue)) = self.inputs.iter_mut().find(|(terminal, _)| terminal == id)
            else {
                return;
            };
            match error {
                Some(error) => {
                    if !queue.fail(sequence, applied) {
                        return;
                    }
                    Err(error)
                }
                None => {
                    if !queue.succeed(sequence) {
                        return;
                    }
                    Ok((queue.should_flush_now(), queue.has_pending()))
                }
            }
        };
        match outcome {
            Err(error) => {
                self.message = error.clone();
                self.input_error = Some((id.clone(), error));
                self.message_from_poll = false;
                if self.input_recovery {
                    // 再同期後の再送も失敗したため、自動再試行を止める。
                    // 未送信入力は保持し、次の入力で再開する。
                    self.input_recovery = false;
                } else {
                    self.input_recovery = true;
                    self.resync_inputs(cx);
                }
            }
            Ok((flush_now, has_pending)) => {
                self.input_recovery = false;
                let matches = matches!(
                    self.input_error.as_ref(),
                    Some((terminal, error)) if terminal == id && error == &self.message
                );
                if matches {
                    self.input_error = None;
                    self.message.clear();
                }
                if flush_now {
                    self.flush_terminal_input(id.clone(), cx);
                } else if has_pending {
                    self.schedule_input_flush(cx);
                }
            }
        }
        cx.notify();
    }

    /// 入力失敗後に端末状態を再同期し、未受理入力を順序どおり再送する。
    fn resync_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        if self.resyncing_inputs {
            return;
        }
        self.resyncing_inputs = true;
        let token = self.requests.begin_action();
        let epoch = self.input_epoch;
        let admitted = self.worker.submit(
            cx,
            |sessions| {
                let outcome = sessions.refresh().map_err(|error| error.to_string());
                Ok((
                    outcome,
                    sessions.version(),
                    sessions.session.terminals().to_vec(),
                    sessions.owners().clone(),
                ))
            },
            move |feature, result, cx| {
                let accepted = feature.requests.finish_action(token);
                if feature.input_epoch != epoch {
                    // コンテキストが切り替わった。フラグは set_context が初期化済み。
                    return;
                }
                // 後続 action（resize / claim / operate 等）に追い越されても、
                // 再同期中フラグと未送信入力の再送は必ず完了させる。
                feature.resyncing_inputs = false;
                if accepted {
                    match result {
                        Ok((Ok(_), version, terminals, owners)) => {
                            feature.apply_projection(version, terminals, owners);
                        }
                        Ok((Err(error), version, terminals, owners)) => {
                            feature.apply_projection(version, terminals, owners);
                            feature.input_recovery = false;
                            feature.message = error;
                            feature.message_from_poll = false;
                            cx.notify();
                            return;
                        }
                        Err(error) => {
                            feature.input_recovery = false;
                            feature.message = error;
                            feature.message_from_poll = false;
                            cx.notify();
                            return;
                        }
                    }
                }
                cx.notify();
                feature.flush_pending_inputs(cx);
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.resyncing_inputs = false;
            self.input_recovery = false;
            self.message = error;
            self.message_from_poll = false;
            cx.notify();
        }
    }

    /// 他の Client が操作中でも、明示的に操作権を奪う。
    pub(super) fn claim(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(id) = self.active_id() else { return };
        self.operate(move |session| session.claim(&id), cx);
    }

    pub(super) fn release(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(id) = self.active_id() else { return };
        self.operate(move |session| session.release(&id), cx);
    }

    pub(super) fn resize(&mut self, columns: u16, rows: u16, cx: &mut gpui::Context<Self>) {
        let Some(id) = self.active_id() else { return };
        self.operate(move |session| session.resize(&id, columns, rows), cx);
    }

    pub(super) fn confirm_terminate(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(id) = self.active_id() else { return };
        let message = format!(
            "{} のshellとCLIを終了します。他のClientもこのTerminalを使用できなくなります。",
            id.as_str()
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Terminalを終了しますか？",
            Some(&message),
            &[
                gpui::PromptButton::new("終了"),
                gpui::PromptButton::cancel("戻る"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update(cx, |this, cx| {
                    this.operate(move |session| session.terminate(&id), cx)
                });
            }
        })
        .detach();
    }

    fn operate(
        &mut self,
        operation: impl FnOnce(&mut TerminalSession) -> Result<(), TerminalError> + Send + 'static,
        cx: &mut gpui::Context<Self>,
    ) {
        let token = self.requests.begin_action();
        let admitted = self.worker.submit(
            cx,
            move |sessions| {
                let (outcome, terminals) = sessions.action(operation);
                Ok((
                    outcome,
                    sessions.version(),
                    terminals,
                    sessions.owners().clone(),
                ))
            },
            move |feature, result, cx| {
                if feature.requests.finish_action(token) {
                    match result {
                        Ok((outcome, version, terminals, owners)) => {
                            feature.apply_projection(version, terminals, owners);
                            feature.message = outcome.err().unwrap_or_default();
                        }
                        Err(error) => feature.message = error,
                    }
                    feature.message_from_poll = false;
                    cx.notify();
                }
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_action(token);
            self.message = error;
            self.message_from_poll = false;
            cx.notify();
        }
    }
    fn poll(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(token) = self.requests.begin_poll() else {
            return;
        };
        let admitted = self.worker.submit(
            cx,
            |sessions| {
                let outcome = sessions.refresh().map_err(|error| error.to_string());
                let owners = sessions.owners().clone();
                let state = (
                    sessions.version(),
                    sessions.session.terminals().to_vec(),
                    owners,
                );
                Ok((outcome, state))
            },
            move |feature, result, cx| {
                if !feature.requests.finish_poll(token) {
                    return;
                }
                match result {
                    Ok((outcome, (version, terminals, owners))) => {
                        feature.apply_projection(version, terminals, owners);
                        match outcome {
                            Ok(_) => {
                                if feature.message_from_poll {
                                    feature.message.clear();
                                    feature.message_from_poll = false;
                                }
                            }
                            Err(error) => {
                                feature.message = error;
                                feature.message_from_poll = true;
                            }
                        }
                        feature.request_output_render(cx);
                    }
                    Err(error) => {
                        feature.message = error;
                        feature.message_from_poll = true;
                        cx.notify();
                    }
                }
            },
        );
        if let Err(error) = admitted {
            self.requests.finish_poll(token);
            self.message = error;
            self.message_from_poll = true;
            cx.notify();
        }
    }
}
