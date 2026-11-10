//! Hostが所有するTerminal状態とClientへ公開する共通Model。

use std::{collections::BTreeMap, fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

pub type TerminalRevision = u64;

pub const MAX_SCROLLBACK_LINES: usize = 2_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalId(String);

impl TerminalId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Running,
    Disconnected,
    Exited,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
/// Host が公開する Terminal profile の識別子。
pub struct TerminalProfileId(String);

impl TerminalProfileId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Host が起動を許可する Terminal profile。
pub struct TerminalProfile {
    pub id: TerminalProfileId,
    pub name: String,
    pub program: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl TerminalProfile {
    pub fn new(id: impl Into<String>, name: impl Into<String>, program: impl Into<String>) -> Self {
        Self {
            id: TerminalProfileId::new(id),
            name: name.into(),
            program: program.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_env(
        mut self,
        env: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.env = env
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// Host が公開する profile 一覧と既定 profile。
pub struct TerminalProfiles {
    pub profiles: Vec<TerminalProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<TerminalProfileId>,
}

impl TerminalProfiles {
    pub fn new(profiles: Vec<TerminalProfile>, default: Option<TerminalProfileId>) -> Self {
        Self { profiles, default }
    }

    pub fn find(&self, id: &TerminalProfileId) -> Option<&TerminalProfile> {
        self.profiles.iter().find(|profile| &profile.id == id)
    }

    /// 要求された profile を解決する。未指定なら既定 profile。
    pub fn resolve(
        &self,
        requested: Option<&TerminalProfileId>,
    ) -> Result<Option<&TerminalProfile>, TerminalError> {
        match requested {
            Some(id) => self.find(id).map(Some).ok_or_else(|| {
                TerminalError::new(format!(
                    "Terminal profile が見つかりません: {}",
                    id.as_str()
                ))
            }),
            None => Ok(self.default.as_ref().and_then(|id| self.find(id))),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub const fn new(columns: u16, rows: u16) -> Self {
        Self { columns, rows }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb {
        red: u8,
        green: u8,
        blue: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCell {
    /// セルの主文字とcombining characterを連結したgrapheme。
    pub text: String,
    /// 端末上の占有幅。wide characterは2、spacerは0。
    pub width: u8,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    pub inverse: bool,
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self {
            text: " ".to_owned(),
            width: 1,
            foreground: TerminalColor::Default,
            background: TerminalColor::Default,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            inverse: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRow {
    pub cells: Vec<TerminalCell>,
    pub wrapped: bool,
}

impl TerminalRow {
    pub fn empty(columns: u16) -> Self {
        Self {
            cells: (0..columns).map(|_| TerminalCell::default()).collect(),
            wrapped: false,
        }
    }

    /// テスト用：指定したテキストで埋められた行を作成する。
    pub fn from_text(columns: u16, text: &str) -> Self {
        let mut row = Self::empty(columns);
        if !text.is_empty() {
            row.cells[0].text = text.to_owned();
        }
        row
    }

    pub fn plain_text(&self) -> String {
        self.cells
            .iter()
            .filter(|cell| cell.width != 0)
            .map(|cell| cell.text.as_str())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalScrollbackDelta {
    pub reset: bool,
    pub dropped: u16,
    pub rows: Vec<TerminalRow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCursor {
    pub row: u16,
    pub column: u16,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalModes {
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalScreen {
    pub size: TerminalSize,
    pub rows: Vec<TerminalRow>,
    pub cursor: TerminalCursor,
    pub modes: TerminalModes,
    #[serde(default)]
    pub scrollback: Vec<TerminalRow>,
}

impl TerminalScreen {
    pub fn empty(size: TerminalSize) -> Self {
        Self {
            size,
            rows: (0..size.rows)
                .map(|_| TerminalRow::empty(size.columns))
                .collect(),
            cursor: TerminalCursor::default(),
            modes: TerminalModes::default(),
            scrollback: Vec::new(),
        }
    }

    pub fn plain_text(&self) -> String {
        self.rows
            .iter()
            .map(TerminalRow::plain_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Deltaを適用して表示用のTerminal stateを更新する。
    /// atomic: validation失敗時は元のstateが完全に変更されない。
    pub fn apply_delta(&mut self, delta: &TerminalDelta) -> Result<(), TerminalError> {
        // 1. validate
        if delta.size.columns == 0
            || delta.size.rows == 0
            || delta.size.columns > 512
            || delta.size.rows > 256
        {
            return Err(TerminalError::new(format!(
                "invalid terminal size: {}x{}",
                delta.size.columns, delta.size.rows
            )));
        }

        // size変更時のvalidation
        let size_changed = self.size != delta.size;
        if size_changed {
            // rows validation
            for update in &delta.changed_rows {
                if update.row >= delta.size.rows
                    || update.value.cells.len() != delta.size.columns as usize
                {
                    return Err(TerminalError::new(format!(
                        "invalid terminal delta revision {}",
                        delta.revision
                    )));
                }
            }
        } else {
            // rows validation
            for update in &delta.changed_rows {
                if update.row >= self.size.rows
                    || update.value.cells.len() != self.size.columns as usize
                {
                    return Err(TerminalError::new(format!(
                        "invalid terminal delta revision {}",
                        delta.revision
                    )));
                }
            }
        }

        // scrollback validation
        if let Some(scrollback) = &delta.scrollback {
            let cols = if size_changed {
                delta.size.columns
            } else {
                self.size.columns
            };
            if scrollback
                .rows
                .iter()
                .any(|row| row.cells.len() != cols as usize)
            {
                return Err(TerminalError::new(format!(
                    "invalid terminal scrollback revision {}",
                    delta.revision
                )));
            }
            if !scrollback.reset {
                let dropped = usize::from(scrollback.dropped);
                if size_changed && dropped > 0 {
                    return Err(TerminalError::new(format!(
                        "terminal scrollback gap at revision {}",
                        delta.revision
                    )));
                }
                if !size_changed && dropped > self.scrollback.len() {
                    return Err(TerminalError::new(format!(
                        "terminal scrollback gap at revision {}",
                        delta.revision
                    )));
                }
                let base_len = if size_changed {
                    0
                } else {
                    self.scrollback.len()
                };
                let new_len = base_len.saturating_sub(dropped) + scrollback.rows.len();
                if new_len > MAX_SCROLLBACK_LINES {
                    return Err(TerminalError::new(format!(
                        "terminal scrollback is too large at revision {}",
                        delta.revision
                    )));
                }
            } else if scrollback.rows.len() > MAX_SCROLLBACK_LINES {
                return Err(TerminalError::new(format!(
                    "terminal scrollback is too large at revision {}",
                    delta.revision
                )));
            }
        }

        // 2. apply（validation済みなので安全）
        if size_changed {
            self.size = delta.size;
            self.rows = (0..delta.size.rows)
                .map(|_| TerminalRow::empty(delta.size.columns))
                .collect();
            self.scrollback.clear();
        }

        for update in &delta.changed_rows {
            self.rows[update.row as usize] = update.value.clone();
        }
        self.cursor = delta.cursor;
        self.modes = delta.modes;
        if let Some(scrollback) = &delta.scrollback {
            if scrollback.reset {
                self.scrollback = scrollback.rows.clone();
            } else {
                let dropped = usize::from(scrollback.dropped);
                self.scrollback.drain(..dropped);
                self.scrollback.extend(scrollback.rows.clone());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRowUpdate {
    pub row: u16,
    pub value: TerminalRow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDelta {
    pub revision: TerminalRevision,
    pub size: TerminalSize,
    pub changed_rows: Vec<TerminalRowUpdate>,
    pub cursor: TerminalCursor,
    pub modes: TerminalModes,
    #[serde(default)]
    pub scrollback: Option<TerminalScrollbackDelta>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub id: TerminalId,
    pub cwd: PathBuf,
    pub status: TerminalStatus,
    pub revision: TerminalRevision,
    pub screen: TerminalScreen,
}

impl TerminalSnapshot {
    pub fn empty(id: TerminalId, cwd: PathBuf, status: TerminalStatus, size: TerminalSize) -> Self {
        Self {
            id,
            cwd,
            status,
            revision: 0,
            screen: TerminalScreen::empty(size),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKey {
    Character(String),
    Enter,
    Backspace,
    Tab,
    Escape,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    Function(u8),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalKeyEvent {
    pub key: TerminalKey,
    pub modifiers: TerminalModifiers,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum TerminalInput {
    Text(String),
    Key(TerminalKeyEvent),
    Paste(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalEvent {
    Delta(TerminalDelta),
    Snapshot(TerminalSnapshot),
    /// 入力できる Session。`None` は誰も操作していない。
    Owner {
        owner: Option<String>,
    },
    Exited {
        code: Option<i32>,
    },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalError(String);

impl TerminalError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for TerminalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TerminalError {}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_resolve_only_ids_that_the_host_published() {
        let profiles = TerminalProfiles::new(
            vec![
                TerminalProfile::new("default", "Default", "sh"),
                TerminalProfile::new("zsh", "zsh", "zsh").with_args(["-l"]),
            ],
            Some(TerminalProfileId::new("default")),
        );
        assert_eq!(
            profiles
                .resolve(None)
                .unwrap()
                .map(|profile| profile.id.as_str()),
            Some("default")
        );
        assert_eq!(
            profiles
                .resolve(Some(&TerminalProfileId::new("zsh")))
                .unwrap()
                .unwrap()
                .args,
            vec!["-l"]
        );
        assert!(
            profiles
                .resolve(Some(&TerminalProfileId::new("missing")))
                .is_err()
        );
    }

    #[test]
    fn terminal_profile_serializes_env_and_args() {
        let profile = TerminalProfile::new("shell", "Shell", "/bin/sh")
            .with_args(["-l", "-i"])
            .with_env([("TERM", "xterm-256color")]);
        let loaded: TerminalProfile =
            serde_json::from_str(&serde_json::to_string(&profile).unwrap()).unwrap();
        assert_eq!(loaded, profile);
    }

    #[test]
    fn terminal_snapshot_round_trips_the_renderer_model() {
        let mut screen = TerminalScreen::empty(TerminalSize::new(120, 36));
        screen.rows[0].cells[0] = TerminalCell {
            text: "あ".to_owned(),
            width: 2,
            foreground: TerminalColor::Rgb {
                red: 1,
                green: 2,
                blue: 3,
            },
            background: TerminalColor::Indexed(4),
            bold: true,
            italic: false,
            underline: true,
            dim: false,
            inverse: false,
        };
        let snapshot = TerminalSnapshot {
            id: TerminalId::new("terminal-1"),
            cwd: PathBuf::from("workspace"),
            status: TerminalStatus::Running,
            revision: 7,
            screen,
        };
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let decoded: TerminalSnapshot = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, snapshot);
        assert_eq!(decoded.screen.rows[0].cells[0].width, 2);
    }

    #[test]
    fn terminal_input_is_semantic_and_lossless_over_json() {
        let input = TerminalInput::Key(TerminalKeyEvent {
            key: TerminalKey::ArrowUp,
            modifiers: TerminalModifiers {
                control: true,
                ..Default::default()
            },
        });
        let encoded = serde_json::to_vec(&input).unwrap();
        assert_eq!(
            serde_json::from_slice::<TerminalInput>(&encoded).unwrap(),
            input
        );
        assert!(!String::from_utf8_lossy(&encoded).contains("\\u001b"));
    }

    #[test]
    fn row_plain_text_skips_wide_character_spacers() {
        let row = TerminalRow {
            cells: vec![
                TerminalCell {
                    text: "界".to_owned(),
                    width: 2,
                    ..Default::default()
                },
                TerminalCell {
                    text: String::new(),
                    width: 0,
                    ..Default::default()
                },
                TerminalCell::default(),
            ],
            wrapped: false,
        };
        assert_eq!(row.plain_text(), "界 ");
    }

    #[test]
    fn terminal_delta_appends_and_truncates_scrollback() {
        let mut screen = TerminalScreen::empty(TerminalSize::new(4, 1));
        screen.scrollback = vec![
            TerminalRow {
                cells: vec![
                    TerminalCell {
                        text: "a".to_owned(),
                        ..Default::default()
                    };
                    4
                ],
                wrapped: false,
            },
            TerminalRow {
                cells: vec![
                    TerminalCell {
                        text: "b".to_owned(),
                        ..Default::default()
                    };
                    4
                ],
                wrapped: false,
            },
        ];
        screen
            .apply_delta(&TerminalDelta {
                revision: 1,
                size: screen.size,
                changed_rows: Vec::new(),
                cursor: screen.cursor,
                modes: screen.modes,
                scrollback: Some(TerminalScrollbackDelta {
                    reset: false,
                    dropped: 1,
                    rows: vec![TerminalRow {
                        cells: vec![
                            TerminalCell {
                                text: "c".to_owned(),
                                ..Default::default()
                            };
                            4
                        ],
                        wrapped: false,
                    }],
                }),
            })
            .unwrap();
        assert_eq!(
            screen
                .scrollback
                .iter()
                .map(TerminalRow::plain_text)
                .collect::<Vec<_>>(),
            vec!["bbbb", "cccc"]
        );

        screen
            .apply_delta(&TerminalDelta {
                revision: 2,
                size: screen.size,
                changed_rows: Vec::new(),
                cursor: screen.cursor,
                modes: screen.modes,
                scrollback: Some(TerminalScrollbackDelta {
                    reset: true,
                    dropped: 0,
                    rows: Vec::new(),
                }),
            })
            .unwrap();
        assert!(screen.scrollback.is_empty());
    }
}
