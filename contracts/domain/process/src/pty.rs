//! OS の PTY に対する機能非依存の契約。認可と Workspace の境界は呼び出し側が所有する。

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

/// Host が所有する PTY の識別子。Host の再起動後は古い ID を使えない。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PtySessionId(pub String);

/// Host が認証済み呼び出し元から割り当てる所有者。拡張からの文字列をそのまま信用しない。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PtyOwnerId(pub String);

/// PTY を共有する接続の識別子。呼び出し元の認証済み ID から割り当てる。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PtyAttachmentId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySize {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySpawn {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    pub size: PtySize,
}

/// `next` を次の購読に渡す。出力を解釈せず byte 列のまま扱う。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyOutput {
    pub bytes: Vec<u8>,
    pub next: u64,
    /// `from` より前の出力が失われた場合、その最初の利用可能な offset。
    pub lost_before: Option<u64>,
    /// 出力を読み終えた後に確定した終了状態。終了コードを得られない場合は None。
    pub exited: Option<PtyExit>,
    /// PTY 読み取りが失敗した場合の詳細。EOF はエラーにしない。
    pub read_error: Option<String>,
    /// 受理済み入力を PTY へ書けなくなった場合の詳細。
    pub write_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyExit {
    pub code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyError(pub String);

impl fmt::Display for PtyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PtyError {}

/// Host 側の同期 port。blocking I/O を含むため UI thread から呼ばない。
pub trait PtyService: Send + Sync {
    fn spawn(&self, owner: &PtyOwnerId, command: PtySpawn) -> Result<PtySessionId, PtyError>;
    fn attach(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: PtyAttachmentId,
    ) -> Result<(), PtyError>;
    fn detach(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
    ) -> Result<(), PtyError>;
    /// 上限付きキューへの受理を返す。実際の書き込み失敗は出力状態で通知する。
    fn write(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        data: &[u8],
    ) -> Result<(), PtyError>;
    fn resize(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        size: PtySize,
    ) -> Result<(), PtyError>;
    fn read(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        from: u64,
        limit: usize,
    ) -> Result<PtyOutput, PtyError>;
    /// 出力または EOF を待つ。timeout 後は空の結果を返す。
    fn wait_read(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        from: u64,
        limit: usize,
        timeout_ms: u64,
    ) -> Result<PtyOutput, PtyError>;
    fn kill(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
    ) -> Result<(), PtyError>;
    /// 終了済みの PTY を回収する。稼働中の PTY は回収しない。
    fn forget(&self, owner: &PtyOwnerId, id: &PtySessionId) -> Result<(), PtyError>;
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_v1_wit_is_valid() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extension-api/wit/pty-v1");
        wit_parser::Resolve::default().push_dir(path).unwrap();
    }
}
