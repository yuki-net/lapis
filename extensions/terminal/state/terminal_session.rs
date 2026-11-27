use serde::{Deserialize, Serialize};
use std::{path::Path, path::PathBuf, sync::Arc};
use terminal::{
    TerminalBackend, TerminalError, TerminalId, TerminalInput, TerminalSnapshot, TerminalStatus,
};
use terminal::{TerminalEvent, TerminalSize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredTerminal {
    #[serde(default)]
    pub id: Option<TerminalId>,
    pub cwd: PathBuf,
    pub status: TerminalStatus,
    pub columns: u16,
    pub rows: u16,
}

pub struct TerminalSession {
    backend: Arc<dyn TerminalBackend>,
    terminals: Vec<TerminalSnapshot>,
    owners: std::collections::HashMap<String, Option<String>>,
}

/// 入力 batch の適用結果。`applied` 件は受理済みなので再送してはいけない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalInputFailure {
    pub message: String,
    pub applied: usize,
}

impl TerminalSession {
    pub fn new(backend: Arc<dyn TerminalBackend>) -> Self {
        Self {
            backend,
            terminals: Vec::new(),
            owners: std::collections::HashMap::new(),
        }
    }

    /// 入力できる Session。`None` は不明または未所有。
    pub fn owner(&self, id: &TerminalId) -> Option<&str> {
        self.owners.get(id.as_str()).and_then(Option::as_deref)
    }

    pub fn owners(&self) -> &std::collections::HashMap<String, Option<String>> {
        &self.owners
    }

    pub fn terminals(&self) -> &[TerminalSnapshot] {
        &self.terminals
    }

    /// 保存済みidentityだけを再アタッチする。shellやCLIを再実行しない。
    pub fn restore_summaries(&mut self, terminals: &[RestoredTerminal]) {
        self.terminals = terminals
            .iter()
            .enumerate()
            .map(|(index, terminal)| TerminalSnapshot {
                id: terminal
                    .id
                    .clone()
                    .unwrap_or_else(|| TerminalId::new(format!("restored-terminal-{}", index + 1))),
                cwd: terminal.cwd.clone(),
                status: if terminal.id.is_some() {
                    TerminalStatus::Disconnected
                } else {
                    TerminalStatus::Exited
                },
                revision: 0,
                screen: terminal::TerminalScreen::empty(TerminalSize::new(
                    terminal.columns,
                    terminal.rows,
                )),
            })
            .collect();
    }

    pub fn terminate_all(&mut self) -> Result<(), TerminalError> {
        let running = self
            .terminals
            .iter()
            .filter(|terminal| terminal.status == TerminalStatus::Running)
            .map(|terminal| terminal.id.clone())
            .collect::<Vec<_>>();
        for id in running {
            self.terminate(&id)?;
        }
        Ok(())
    }

    pub fn start(
        &mut self,
        cwd: &Path,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalId, TerminalError> {
        self.start_profile(cwd, columns, rows, None)
    }

    /// Host が許可した profile で Terminal を開始する。
    pub fn start_profile(
        &mut self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<terminal::TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        let id = self
            .backend
            .start_profile(cwd, columns, rows, profile.as_ref())?;
        if let Ok(snapshot) = self.backend.attach(cwd, &id) {
            self.terminals.push(snapshot);
            return Ok(id);
        }
        self.terminals.push(TerminalSnapshot {
            id: id.clone(),
            cwd: cwd.to_owned(),
            status: TerminalStatus::Running,
            revision: 0,
            screen: terminal::TerminalScreen::empty(TerminalSize::new(columns, rows)),
        });
        Ok(id)
    }

    /// Host が許可する profile 一覧。
    pub fn profiles(&self, cwd: &Path) -> Result<terminal::TerminalProfiles, TerminalError> {
        self.backend.profiles(cwd)
    }

    pub fn discover(&self, cwd: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        self.backend.list(cwd)
    }

    pub fn attach(&mut self, cwd: &Path, id: &TerminalId) -> Result<(), TerminalError> {
        let snapshot = self.backend.attach(cwd, id)?;
        if let Some(existing) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.id == *id)
        {
            *existing = snapshot;
        } else {
            self.terminals.push(snapshot);
        }
        Ok(())
    }

    pub fn input(&self, id: &TerminalId, text: &str) -> Result<(), TerminalError> {
        self.input_semantic(id, TerminalInput::Text(text.to_owned()))
    }

    pub fn input_semantic(
        &self,
        id: &TerminalId,
        input: TerminalInput,
    ) -> Result<(), TerminalError> {
        self.backend.input(id, &input)
    }

    /// 端末の入力 batch を順序どおりに送る。
    ///
    /// 失敗した入力は未受理として扱い、それ以前に受理した件数を返す。
    /// 呼び出し側は `applied` 以降だけを再送し、受理済み入力を重複させない。
    pub fn input_batch(
        &self,
        id: &TerminalId,
        inputs: &[TerminalInput],
    ) -> Result<(), TerminalInputFailure> {
        for (applied, input) in inputs.iter().enumerate() {
            if let Err(error) = self.backend.input(id, input) {
                return Err(TerminalInputFailure {
                    message: error.to_string(),
                    applied,
                });
            }
        }
        Ok(())
    }

    pub fn resize(
        &mut self,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<(), TerminalError> {
        // 画面の反映は delta イベントで届くため、ここでは何も変更しない。
        self.backend.resize(id, columns, rows)
    }

    pub fn claim(&mut self, id: &TerminalId) -> Result<(), TerminalError> {
        self.backend.claim(id)
    }

    pub fn release(&mut self, id: &TerminalId) -> Result<(), TerminalError> {
        self.backend.release(id)
    }

    pub fn refresh(&mut self) -> Result<bool, TerminalError> {
        let mut changed = false;
        let mut attach_error = None;
        let mut owner_updates = Vec::new();
        for terminal in &mut self.terminals {
            if terminal.status == TerminalStatus::Disconnected {
                match self.backend.attach(&terminal.cwd, &terminal.id) {
                    Ok(snapshot) => {
                        *terminal = snapshot;
                        changed = true;
                    }
                    Err(error) => {
                        attach_error.get_or_insert_with(|| error.to_string());
                        continue;
                    }
                }
            }
            if terminal.status != TerminalStatus::Running {
                continue;
            }
            let mut resync = false;
            let events = match self.backend.poll(&terminal.id) {
                Ok(events) => events,
                Err(error) => {
                    attach_error.get_or_insert_with(|| error.to_string());
                    break;
                }
            };
            for event in events {
                match event {
                    TerminalEvent::Snapshot(snapshot) => {
                        if snapshot.id != terminal.id || snapshot.revision < terminal.revision {
                            continue;
                        }
                        *terminal = snapshot;
                    }
                    TerminalEvent::Delta(delta) => {
                        if delta.revision <= terminal.revision {
                            continue;
                        }
                        if delta.revision != terminal.revision.saturating_add(1) {
                            resync = true;
                            break;
                        }
                        if terminal.screen.apply_delta(&delta).is_err() {
                            resync = true;
                            break;
                        }
                        terminal.revision = delta.revision;
                    }
                    TerminalEvent::Owner { owner } => {
                        owner_updates.push((terminal.id.as_str().to_owned(), owner));
                    }
                    TerminalEvent::Exited { .. } => terminal.status = TerminalStatus::Exited,
                    TerminalEvent::Failed(_) => {
                        terminal.status = TerminalStatus::Failed;
                    }
                }
                changed = true;
            }
            if resync {
                let id = terminal.id.clone();
                let cwd = terminal.cwd.clone();
                let snapshot = match self.backend.attach(&cwd, &id) {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        attach_error.get_or_insert_with(|| error.to_string());
                        break;
                    }
                };
                if snapshot.id != id || snapshot.revision < terminal.revision {
                    attach_error
                        .get_or_insert_with(|| "terminal snapshot resync failed".to_owned());
                    break;
                }
                *terminal = snapshot;
                changed = true;
            }
        }
        for (id, owner) in owner_updates {
            self.owners.insert(id, owner);
        }
        match attach_error {
            Some(error) => Err(TerminalError::new(error)),
            None => Ok(changed),
        }
    }

    pub fn terminate(&mut self, id: &TerminalId) -> Result<(), TerminalError> {
        self.backend.terminate(id)?;
        if let Some(terminal) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.id == *id)
        {
            terminal.status = TerminalStatus::Exited;
        }
        Ok(())
    }

    /// Tab を閉じたときの終了。実行中なら PTY を終了し、状態からも取り除く。
    ///
    /// 終了に失敗しても表示は取り除く。閉じたタブを復活させず、利用者へ
    /// エラーだけを返すためである。Workspace の detach とは別操作で、
    /// こちらは `terminate` を伴う。
    pub fn terminate_and_forget(&mut self, id: &TerminalId) -> Result<(), TerminalError> {
        let terminal = self
            .terminals
            .iter()
            .find(|terminal| terminal.id == *id)
            .cloned();
        let outcome = match terminal.as_ref().map(|terminal| terminal.status) {
            Some(TerminalStatus::Running) => self.backend.terminate(id),
            // 復元直後の未接続 Terminal は、終了の前に再接続して Host の PTY を掴む。
            Some(TerminalStatus::Disconnected) => {
                let cwd = terminal.as_ref().map(|terminal| terminal.cwd.clone());
                if let Some(cwd) = cwd
                    && let Err(error) = self.backend.attach(&cwd, id)
                {
                    Err(error)
                } else {
                    self.backend.terminate(id)
                }
            }
            // 既に終了・失敗した Terminal は PTY を重ねて終了しない。
            _ => Ok(()),
        };
        self.terminals.retain(|terminal| terminal.id != *id);
        self.owners.remove(id.as_str());
        outcome
    }

    pub fn backend(&self) -> Arc<dyn TerminalBackend> {
        self.backend.clone()
    }

    /// テストで状態を直接組み立てるための可変参照。
    #[cfg(test)]
    pub fn terminals_mut_for_tests(&mut self) -> &mut Vec<TerminalSnapshot> {
        &mut self.terminals
    }
}
