//! Terminal 拡張の版付き DTO と汎用拡張 transport の接着部。

use client_api::{HostExtensionDispatchRequest, HostExtensionDispatchResponse};
use serde::{Serialize, de::DeserializeOwned};

pub use terminal_session_api::v1;

pub const EXTENSION_ID: &str = "lapis.terminal";

/// 各 Terminal の生出力を一度だけ、順番に画面へ適用するための位置。
pub struct OutputCursor {
    id: String,
    offset: u64,
    generation: u64,
    pending: Option<(u64, u32, Option<u64>)>,
}

/// 接続変更・要求取消後の古い応答を判別する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadTicket(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    Pending,
    Stale,
    InvalidRequest,
    InvalidResponse,
    /// 画面を再構築する必要があり、部分的な出力を適用してはいけない。
    OutputLost {
        expected: u64,
        available_from: u64,
    },
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => formatter.write_str("Terminal の出力取得は既に実行中です"),
            Self::Stale => formatter.write_str("古い Terminal の出力を破棄しました"),
            Self::InvalidRequest => formatter.write_str("Terminal の出力取得条件が不正です"),
            Self::InvalidResponse => formatter.write_str("Terminal の出力応答が不正です"),
            Self::OutputLost {
                expected,
                available_from,
            } => write!(
                formatter,
                "Terminal の出力が欠落しました: {expected}..{available_from}"
            ),
        }
    }
}

impl OutputCursor {
    pub fn new(id: String) -> Self {
        Self {
            id,
            offset: 0,
            generation: 0,
            pending: None,
        }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn begin(
        &mut self,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<(ReadTicket, v1::ReadRequest), ReadError> {
        if self.pending.is_some() {
            return Err(ReadError::Pending);
        }
        if max_bytes == 0 || max_bytes > v1::MAX_READ_BYTES || timeout_ms > v1::MAX_READ_WAIT_MS {
            return Err(ReadError::InvalidRequest);
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending = Some((self.generation, max_bytes, None));
        Ok((
            ReadTicket(self.generation),
            v1::ReadRequest {
                id: self.id.clone(),
                offset: self.offset,
                max_bytes,
                timeout_ms,
            },
        ))
    }

    /// 未完了要求を無効にする。表示済みの出力位置は保持する。
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// 応答を検証する。画面への反映後に commit が必要。
    pub fn accept(
        &mut self,
        ticket: ReadTicket,
        output: v1::ReadResponse,
    ) -> Result<v1::ReadResponse, ReadError> {
        let Some((generation, max_bytes, ready)) = self.pending else {
            return Err(ReadError::Stale);
        };
        if generation != ticket.0 || ready.is_some() {
            return Err(ReadError::Stale);
        }
        self.pending = None;
        if let Some(available_from) = output.lost_before {
            if available_from <= self.offset
                || output.bytes.len() > max_bytes as usize
                || available_from.checked_add(output.bytes.len() as u64) != Some(output.next)
            {
                return Err(ReadError::InvalidResponse);
            }
            return Err(ReadError::OutputLost {
                expected: self.offset,
                available_from,
            });
        }
        let next = self
            .offset
            .checked_add(output.bytes.len() as u64)
            .ok_or(ReadError::InvalidResponse)?;
        if output.next != next || output.bytes.len() > max_bytes as usize {
            return Err(ReadError::InvalidResponse);
        }
        self.pending = Some((generation, max_bytes, Some(next)));
        Ok(output)
    }

    /// 検証済みの出力を描画状態へ反映できたときだけ位置を確定する。
    pub fn commit(&mut self, ticket: ReadTicket) -> Result<(), ReadError> {
        match self.pending {
            Some((generation, _, Some(next))) if generation == ticket.0 => {
                self.offset = next;
                self.pending = None;
                Ok(())
            }
            _ => Err(ReadError::Stale),
        }
    }

    pub fn ready(&self, ticket: ReadTicket) -> bool {
        matches!(self.pending, Some((generation, _, Some(_))) if generation == ticket.0)
    }

    pub fn discard(&mut self, ticket: ReadTicket) -> Result<(), ReadError> {
        if !matches!(self.pending, Some((generation, _, _)) if generation == ticket.0) {
            return Err(ReadError::Stale);
        }
        self.pending = None;
        Ok(())
    }
}

/// 呼び出し元は v1 の操作名と対応する要求 DTO を指定する。
pub fn request<T: Serialize>(
    operation: &str,
    value: &T,
) -> Result<HostExtensionDispatchRequest, String> {
    Ok(HostExtensionDispatchRequest {
        id: EXTENSION_ID.to_owned(),
        operation: operation.to_owned(),
        payload: serde_json::to_string(value).map_err(|error| error.to_string())?,
    })
}

pub fn response<T: DeserializeOwned>(
    operation: &str,
    response: HostExtensionDispatchResponse,
) -> Result<T, String> {
    if response.id != EXTENSION_ID || response.operation != operation {
        return Err("Terminal 拡張の応答元が一致しません".into());
    }
    serde_json::from_str(&response.result).map_err(|_| "Terminal 拡張の応答が不正です".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_wire_codec_checks_identity_operation_and_body() {
        let request = request(v1::LIST, &v1::ListRequest {}).unwrap();
        assert_eq!(request.id, EXTENSION_ID);
        assert_eq!(request.operation, v1::LIST);
        assert_eq!(request.payload, "{}");
        let reply = HostExtensionDispatchResponse {
            id: request.id,
            operation: request.operation,
            result: r#"{"sessions":[]}"#.into(),
        };
        assert_eq!(
            response::<v1::ListResponse>(v1::LIST, reply.clone())
                .unwrap()
                .sessions
                .len(),
            0
        );
        assert!(response::<v1::ListResponse>(v1::READ, reply.clone()).is_err());
        let mut wrong = reply.clone();
        wrong.id = "other".into();
        assert!(response::<v1::ListResponse>(v1::LIST, wrong).is_err());
        let mut wrong = reply;
        wrong.result = "invalid json".into();
        assert!(response::<v1::ListResponse>(v1::LIST, wrong).is_err());
    }

    fn output(bytes: &[u8], next: u64, lost_before: Option<u64>) -> v1::ReadResponse {
        v1::ReadResponse {
            bytes: bytes.to_vec(),
            next,
            lost_before,
            exit: None,
            read_error: None,
            write_error: None,
        }
    }

    #[test]
    fn output_cursor_rejects_stale_lost_and_malformed_output() {
        let mut cursor = OutputCursor::new("pty-1".into());
        let (first, request) = cursor.begin(32, 100).unwrap();
        assert_eq!((request.id.as_str(), request.offset), ("pty-1", 0));
        assert!(matches!(cursor.begin(32, 100), Err(ReadError::Pending)));
        cursor.cancel();
        let (second, _) = cursor.begin(32, 100).unwrap();
        assert_eq!(
            cursor.accept(first, output(b"old", 3, None)),
            Err(ReadError::Stale)
        );
        assert_eq!(
            cursor
                .accept(second, output(b"abc", 3, None))
                .unwrap()
                .bytes,
            b"abc"
        );
        assert_eq!(cursor.offset(), 0);
        assert!(matches!(cursor.begin(32, 100), Err(ReadError::Pending)));
        assert_eq!(cursor.commit(first), Err(ReadError::Stale));
        cursor.commit(second).unwrap();
        assert_eq!(cursor.offset(), 3);
        let (third, request) = cursor.begin(32, 100).unwrap();
        assert_eq!(request.offset, 3);
        assert_eq!(
            cursor.accept(third, output(b"x", 9, Some(8))),
            Err(ReadError::OutputLost {
                expected: 3,
                available_from: 8
            })
        );
        assert_eq!(cursor.offset(), 3);
        let (fourth, _) = cursor.begin(32, 100).unwrap();
        assert_eq!(
            cursor.accept(fourth, output(b"x", 10, Some(8))),
            Err(ReadError::InvalidResponse)
        );
        assert_eq!(cursor.offset(), 3);
        let (fifth, _) = cursor.begin(32, 100).unwrap();
        assert_eq!(
            cursor.accept(fifth, output(b"de", 5, None)).unwrap().bytes,
            b"de"
        );
        cursor.commit(fifth).unwrap();
        assert_eq!(cursor.offset(), 5);
        assert!(matches!(cursor.begin(0, 0), Err(ReadError::InvalidRequest)));
        assert!(matches!(
            cursor.begin(1, v1::MAX_READ_WAIT_MS + 1),
            Err(ReadError::InvalidRequest)
        ));
        let (sixth, _) = cursor.begin(1, 0).unwrap();
        assert_eq!(
            cursor.accept(sixth, output(b"fg", 7, None)),
            Err(ReadError::InvalidResponse)
        );
        assert_eq!(cursor.offset(), 5);
        let (seventh, _) = cursor.begin(2, 0).unwrap();
        cursor.accept(seventh, output(b"fg", 7, None)).unwrap();
        cursor.cancel();
        assert_eq!(cursor.commit(seventh), Err(ReadError::Stale));
        assert_eq!(cursor.offset(), 5);
        assert_eq!(cursor.begin(2, 0).unwrap().1.offset, 5);
    }
}
