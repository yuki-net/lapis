#[derive(Default)]
pub(super) struct TerminalRequests {
    pub(super) gate: crate::client_adapter::ResponseGate,
    poll_gate: crate::client_adapter::ResponseGate,
    poll_pending: bool,
    pub(super) pending_actions: usize,
    pub(super) context_unavailable: bool,
}

/// ターミナルタブを開いたときの自動起動の進み具合。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum AutoStart {
    /// まだ開始していない。タブが見えていれば開始してよい。
    #[default]
    Idle,
    /// 起動を試行済み。タブを閉じるかコンテキストが変わるまで再試行しない。
    Finished,
}

impl TerminalRequests {
    pub(super) fn begin_action(&mut self) -> crate::client_worker::RequestToken {
        self.pending_actions += 1;
        self.gate.issue()
    }

    pub(super) fn finish_action(&mut self, token: crate::client_worker::RequestToken) -> bool {
        self.pending_actions = self.pending_actions.saturating_sub(1);
        self.gate.accept(token)
    }

    pub(super) fn begin_poll(&mut self) -> Option<crate::client_worker::RequestToken> {
        // 応答は受理だけなので、進行中の action が画面取得を妨げない。
        // poll は専用の gate を使い、action の結果を上書きしない。
        if self.context_unavailable || self.poll_pending {
            return None;
        }
        self.poll_pending = true;
        Some(self.poll_gate.issue())
    }

    pub(super) fn finish_poll(&mut self, token: crate::client_worker::RequestToken) -> bool {
        // 古い応答で新しい poll の pending を解除しない。
        if !self.poll_gate.accept(token) {
            return false;
        }
        self.poll_pending = false;
        true
    }

    /// context の切替などで、進行中の action と poll の両方を破棄する。
    pub(super) fn invalidate(&mut self) {
        self.gate.invalidate();
        self.poll_gate.invalidate();
        self.poll_pending = false;
    }
}
