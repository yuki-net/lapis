//! Terminal 拡張の契約。
//!
//! `TerminalBackend` port を定義し、型は `protocol/types/terminal` を再エクスポートする。
//!
//! 参照側は crate 名 `terminal`（package `terminal-contract`）で依存する。

pub use terminal_types::*;

pub trait TerminalBackend: Send + Sync {
    fn start(
        &self,
        cwd: &std::path::Path,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalId, TerminalError>;
    /// 指定された profile で Terminal を開始する。`None` は Host の既定 profile。
    fn start_profile(
        &self,
        cwd: &std::path::Path,
        columns: u16,
        rows: u16,
        profile: Option<&TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        if profile.is_some() {
            return Err(TerminalError::new("この接続ではprofileを指定できません"));
        }
        self.start(cwd, columns, rows)
    }
    /// Host が許可する profile 一覧。未対応の backend は空。
    fn profiles(&self, _cwd: &std::path::Path) -> Result<TerminalProfiles, TerminalError> {
        Ok(TerminalProfiles::default())
    }
    fn list(&self, _cwd: &std::path::Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        Err(TerminalError::new(
            "この接続ではTerminal一覧を取得できません",
        ))
    }
    fn attach(
        &self,
        _cwd: &std::path::Path,
        _id: &TerminalId,
    ) -> Result<TerminalSnapshot, TerminalError> {
        Err(TerminalError::new(
            "この接続ではTerminalを再アタッチできません",
        ))
    }
    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError>;
    /// Client 視点の resize。画面の反映はイベントで届く。
    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError>;
    /// Host 側エミュレータが resize を適用し、配布する差分を返す。
    ///
    /// 既定実装は差分を生成できない。リモート接続の Client は `resize` だけを使う。
    fn resize_delta(
        &self,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalDelta, TerminalError> {
        let _ = (id, columns, rows);
        Err(TerminalError::new(
            "この接続ではTerminalの画面差分を生成できません",
        ))
    }
    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError>;
    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError>;
    /// OS の root PID。Resource Manager へ所有ツリーを登録するために使う。
    ///
    /// 別プロセス上の backend など、PID を観測できない実装は `None` を返す。
    fn process_id(&self, id: &TerminalId) -> Option<u32> {
        let _ = id;
        None
    }
    /// 入力できる操作権を取得する。他の Session が所有中なら奪う。
    fn claim(&self, id: &TerminalId) -> Result<(), TerminalError> {
        let _ = id;
        Err(TerminalError::new(
            "この接続ではTerminalの操作権を取得できません",
        ))
    }
    /// 操作権を手放す。
    fn release(&self, id: &TerminalId) -> Result<(), TerminalError> {
        let _ = id;
        Err(TerminalError::new(
            "この接続ではTerminalの操作権を手放せません",
        ))
    }

    /// 画面や状態が変わったときに通知する。未対応の backend は `None`。
    ///
    /// receiver は 1 度だけ取得できる。UI はこの通知を受けて `poll` を呼ぶ。
    fn changes(&self) -> Option<futures_channel::mpsc::UnboundedReceiver<()>> {
        None
    }
}
