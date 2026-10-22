use super::*;
use lapis_git::{ConflictSide, GitAction, GitActionRequest, GitActionResult};

/// 全Git操作を非対話で実行し、期限超過時は子プロセスも終了する。
pub(super) fn git_output(
    command: &mut Command,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::{Seek, SeekFrom};
    // pipeを保持する認証ヘルパーを待ち続けないよう、出力は一時ファイルへ捕捉する。
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    if env::var_os("GIT_SSH_COMMAND").is_none() && env::var_os("GIT_SSH").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(10)),
            result => {
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    let _ = Command::new("taskkill")
                        .args(["/F", "/T", "/PID", &child.id().to_string()])
                        .creation_flags(0x08000000)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                #[cfg(unix)]
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                return Err(result.err().unwrap_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Git操作が120秒以内に完了しませんでした。状態を更新して確認してください",
                    )
                }));
            }
        }
    };
    stdout.seek(SeekFrom::Start(0))?;
    stderr.seek(SeekFrom::Start(0))?;
    let mut output = std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    stdout.read_to_end(&mut output.stdout)?;
    stderr.read_to_end(&mut output.stderr)?;
    Ok(output)
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::Conflict, message)
}

fn name(value: &str) -> Result<&str, GitError> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.contains(['\0', '\r', '\n'])
    {
        return Err(invalid("Branch名が不正です"));
    }
    Ok(value)
}

fn relative(value: &str) -> Result<&str, GitError> {
    if value.is_empty()
        || value.contains('\0')
        || Path::new(value)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(invalid("Repository内の相対pathを指定してください"));
    }
    Ok(value)
}

fn diff(repository: &Path, path: &str, staged: bool) -> Result<String, GitError> {
    let path = relative(path)?;
    if !staged && git_text(repository, &["ls-files", "--error-unmatch", "--", path]).is_err() {
        return Ok(String::from_utf8_lossy(&git_bytes_allow_exit_code(
            repository,
            &[
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--binary",
                "--",
                "/dev/null",
                path,
            ],
            1,
        )?)
        .into_owned());
    }
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--binary",
    ];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", path]);
    git_text(repository, &args)
}

fn stash(
    repository: &Path,
    operation: &str,
    index: u32,
    expected_oid: &str,
) -> Result<String, GitError> {
    let reference = format!("stash@{{{index}}}");
    let oid = git_text(repository, &["rev-parse", "--verify", &reference])?;
    if expected_oid.is_empty() || oid.trim() != expected_oid {
        return Err(invalid(
            "Stash一覧が変わりました。更新して選び直してください",
        ));
    }
    git_text(repository, &["stash", operation, &reference])
}

fn selected_hunk(patch: &str, index: usize) -> Result<String, GitError> {
    let lines = patch.split_inclusive('\n').collect::<Vec<_>>();
    let hunks = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("@@ ").then_some(index))
        .collect::<Vec<_>>();
    let start = *hunks
        .get(index)
        .ok_or_else(|| invalid("部分Stageのhunkがありません"))?;
    let end = hunks.get(index + 1).copied().unwrap_or(lines.len());
    if lines[start..end]
        .iter()
        .any(|line| line.starts_with("diff --git "))
    {
        return Err(invalid("一つのファイルのhunkを選択してください"));
    }
    Ok(lines[..hunks[0]]
        .iter()
        .chain(&lines[start..end])
        .copied()
        .collect())
}

fn apply_patch(repository: &Path, patch: String, reverse: bool) -> Result<(), GitError> {
    use std::io::{Seek, SeekFrom};
    let mut input = tempfile::tempfile().map_err(|error| invalid(error.to_string()))?;
    input
        .write_all(patch.as_bytes())
        .map_err(|error| invalid(error.to_string()))?;
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| invalid(error.to_string()))?;
    let mut command = Command::new("git");
    command.current_dir(repository).args([
        "-c",
        "credential.interactive=never",
        "apply",
        "--cached",
        "--whitespace=nowarn",
    ]);
    if reverse {
        command.arg("--reverse");
    }
    let output = git_output(command.stdin(input), Duration::from_secs(120))
        .map_err(|error| invalid(error.to_string()))?;
    if !output.status.success() {
        return Err(invalid(String::from_utf8_lossy(&output.stderr)));
    }
    Ok(())
}

impl LocalGitBackend {
    pub(super) fn execute_action(
        &self,
        repository: &Path,
        request: GitActionRequest,
    ) -> Result<GitActionResult, GitError> {
        let status = self.status(repository)?;
        if request.action.is_mutation() && request.expected_head.as_deref() != Some(&status.head) {
            return Err(invalid(
                "HEADが表示時から変わりました。Statusを更新して操作し直してください",
            ));
        }
        let text = match request.action {
            GitAction::Diff { path, staged } => diff(repository, &path, staged)?,
            GitAction::Stage { path } => git_text(repository, &["add", "--", relative(&path)?])?,
            GitAction::Unstage { path } => {
                let path = relative(&path)?;
                let mut paths = vec![path];
                if let Some(old) = status
                    .files
                    .iter()
                    .find(|file| file.path == Path::new(path) && file.staged)
                    .and_then(|file| file.old_path.as_ref())
                {
                    paths.push(
                        old.to_str()
                            .ok_or_else(|| invalid("Git pathを解釈できません"))?,
                    );
                }
                let mut args = if status.head.is_empty() {
                    vec!["rm", "--cached", "--"]
                } else {
                    vec!["restore", "--staged", "--"]
                };
                args.extend(paths);
                git_text(repository, &args)?
            }
            GitAction::StageHunk {
                path,
                staged,
                hunk,
                expected_patch,
            } => {
                if expected_patch.len() > 1024 * 1024 {
                    return Err(invalid("部分Stageのpatchが大きすぎます"));
                }
                let current = diff(repository, &path, staged)?;
                if current != expected_patch {
                    return Err(invalid(
                        "Diffが変更されました。更新してhunkを選び直してください",
                    ));
                }
                apply_patch(repository, selected_hunk(&current, hunk)?, staged)?;
                String::new()
            }
            GitAction::Commit { message } => {
                if message.trim().is_empty() || message.len() > 64 * 1024 || message.contains('\0')
                {
                    return Err(invalid("Commit messageを入力してください"));
                }
                git_text(
                    repository,
                    &["-c", "core.editor=true", "commit", "-m", &message],
                )?
            }
            GitAction::History => {
                if status.head.is_empty() {
                    String::new()
                } else {
                    git_text(
                        repository,
                        &[
                            "log",
                            "-60",
                            "--date=iso-strict",
                            "--format=%h %ad %an%n%s%n",
                            "--no-decorate",
                        ],
                    )?
                }
            }
            GitAction::Branches => git_text(
                repository,
                &[
                    "branch",
                    "--list",
                    "--format=%(HEAD) %(refname:short) %(objectname:short)",
                ],
            )?,
            GitAction::CreateBranch { name: branch } => {
                git_text(
                    repository,
                    &["check-ref-format", "--branch", name(&branch)?],
                )?;
                git_text(repository, &["branch", "--", &branch])?
            }
            GitAction::SwitchBranch { name: branch } => {
                git_text(repository, &["switch", "--", name(&branch)?])?
            }
            GitAction::DeleteBranch { name: branch } => {
                git_text(repository, &["branch", "-d", "--", name(&branch)?])?
            }
            GitAction::Fetch => git_text(
                repository,
                &[
                    "-c",
                    "credential.interactive=never",
                    "fetch",
                    "--all",
                    "--prune",
                ],
            )?,
            GitAction::Pull => git_text(
                repository,
                &[
                    "-c",
                    "credential.interactive=never",
                    "pull",
                    "--ff-only",
                    "--no-edit",
                ],
            )?,
            GitAction::Push => {
                if git_text(repository, &["rev-parse", "--verify", "@{upstream}"]).is_ok() {
                    git_text(repository, &["-c", "credential.interactive=never", "push"])?
                } else {
                    git_text(
                        repository,
                        &[
                            "-c",
                            "credential.interactive=never",
                            "push",
                            "--set-upstream",
                            "--",
                            "origin",
                            &format!("refs/heads/{}", status.branch),
                        ],
                    )?
                }
            }
            GitAction::Stashes => git_text(repository, &["stash", "list", "--format=%gd %H %gs"])?,
            GitAction::StashPush { message } => {
                if message.len() > 4096 || message.contains('\0') {
                    return Err(invalid("Stash messageが不正です"));
                }
                git_text(
                    repository,
                    &["stash", "push", "--include-untracked", "-m", &message],
                )?
            }
            GitAction::StashApply {
                index,
                expected_oid,
            } => stash(repository, "apply", index, &expected_oid)?,
            GitAction::StashPop {
                index,
                expected_oid,
            } => stash(repository, "pop", index, &expected_oid)?,
            GitAction::StashDrop {
                index,
                expected_oid,
            } => stash(repository, "drop", index, &expected_oid)?,
            GitAction::Merge { branch } => git_text(
                repository,
                &[
                    "-c",
                    "core.editor=true",
                    "merge",
                    "--no-edit",
                    "--",
                    name(&branch)?,
                ],
            )?,
            GitAction::AbortMerge => git_text(repository, &["merge", "--abort"])?,
            GitAction::Resolve { path, side } => {
                let path = relative(&path)?;
                if !status
                    .files
                    .iter()
                    .any(|file| file.path == Path::new(path) && file.kind == ChangeKind::Conflicted)
                {
                    return Err(invalid("競合中のファイルを選択してください"));
                }
                git_text(
                    repository,
                    &[
                        "checkout",
                        match side {
                            ConflictSide::Ours => "--ours",
                            ConflictSide::Theirs => "--theirs",
                        },
                        "--",
                        path,
                    ],
                )?
            }
        };
        if text.len() > 1024 * 1024 {
            return Err(invalid(
                "表示結果が1MiBを超えています。TerminalのGit CLIで確認してください",
            ));
        }
        Ok(GitActionResult { output: text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_stops_a_command_without_waiting_for_its_normal_exit() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30"]);
            command
        };
        let started = Instant::now();
        let error =
            git_output(command.stdin(Stdio::null()), Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn git_output_captures_stdout_stderr_and_exit_code() {
        let output = git_output(
            Command::new("git").arg("--version").stdin(Stdio::null()),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.starts_with(b"git version"));
        assert!(output.stderr.is_empty());
    }
}
