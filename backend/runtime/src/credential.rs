//! 永続化する Host credential の正本表現。

use serde::{Deserialize, Serialize};

/// digest だけを保持し、secret そのものは保存しない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredential {
    pub id: String,
    pub client_id: String,
    pub workspace_id: String,
    pub capabilities: Vec<String>,
    pub digest: String,
    pub expires_at: u64,
}
