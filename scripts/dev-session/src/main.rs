use std::{
    env,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Host の起動を待てる程度にし、無いまま固まらない。
const HOST_FILE_TIMEOUT: Duration = Duration::from_secs(15);
const HOST_FILE_POLL: Duration = Duration::from_millis(100);

/// 起動するもの。`HotReload` は Desktop の起動方法だけを差し替える。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    HostOnly,
    Desktop,
    HotReload,
}

/// 起動モードに応じて Host、Desktop、Hot Reload のいずれかを起動する。
fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("worktree root is unavailable")?;
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    let mode = mode(&arguments)?;
    // ランチャー自身のフラグはアプリへ渡さない。
    let arguments = without_flags(&arguments, &["--hot-reload", "--host-only"]);
    if mode == Mode::HotReload {
        return hot_reload(root, &arguments);
    }
    // `--host-only` は Desktop を起動しないため、Host だけをビルドする。
    // Desktop のビルド失敗で Web 用 Host が起動できなくなるのを避ける。
    let host_only = mode == Mode::HostOnly;
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let build = Command::new(cargo)
        .current_dir(root)
        .arg(if host_only { "host-build" } else { "ide-build" })
        .status()?;
    if !build.success() {
        return Ok(build.code().unwrap_or(1));
    }

    #[cfg(windows)]
    let job = job::SessionJob::new()?;
    let session = create_session(&root.join("target/dev-sessions"))?;
    let bin = session.join("bin");
    fs::create_dir(&bin)?;
    copy_binary(root, &bin, "lapis-host")?;
    let status = if host_only {
        let mut host = Command::new(bin.join(format!("lapis-host{}", env::consts::EXE_SUFFIX)));
        host.current_dir(root)
            .arg("--workspace")
            .arg(root)
            .arg("--certificate")
            .arg(session.join("host.der"))
            .args(&arguments);
        eprintln!(
            "Host session: {}",
            session.file_name().unwrap().to_string_lossy()
        );
        host.status()?
    } else {
        let desktop = copy_binary(root, &bin, "lapis")?;
        let hosts = session.join("hosts");
        fs::create_dir(&hosts)?;
        // Host を別の Run で起動する場合は、招待が現れるまで待ってから接続する。
        wait_for_host_file(root, &arguments)?;
        eprintln!(
            "Desktop session: {}",
            session.file_name().unwrap().to_string_lossy()
        );
        let mut command = Command::new(desktop);
        command
            .current_dir(root)
            .arg("--state-directory")
            .arg(session.join("state"))
            .arg("--workspace")
            .arg(root);
        if !arguments.iter().any(|arg| arg == "--local-host-directory") {
            command.arg("--local-host-directory").arg(hosts);
        }
        // 利用者が後置した指定が優先される。
        command.args(&arguments).status()?
    };
    #[cfg(windows)]
    job.disarm()?;
    Ok(status.code().unwrap_or(1))
}

/// ランチャーのフラグから起動モードを決める。同時指定は受け付けない。
fn mode(arguments: &[OsString]) -> Result<Mode, Box<dyn std::error::Error>> {
    match (
        has_flag(arguments, "--hot-reload"),
        has_flag(arguments, "--host-only"),
    ) {
        (true, true) => Err("--hot-reload と --host-only は同時に指定できません".into()),
        (true, false) => Ok(Mode::HotReload),
        (false, true) => Ok(Mode::HostOnly),
        (false, false) => Ok(Mode::Desktop),
    }
}

/// 指定したフラグが引数に含まれるかを返す。
fn has_flag(arguments: &[OsString], flag: &str) -> bool {
    arguments
        .iter()
        .any(|argument| argument.as_os_str() == OsStr::new(flag))
}

/// ランチャー自身のフラグを取り除いた引数を返す。
fn without_flags(arguments: &[OsString], flags: &[&str]) -> Vec<OsString> {
    arguments
        .iter()
        .filter(|argument| {
            !flags
                .iter()
                .any(|flag| argument.as_os_str() == OsStr::new(flag))
        })
        .cloned()
        .collect()
}

/// Desktop を修正版 dx で Hot Reload 起動する。Windows x64 専用。
fn hot_reload(root: &Path, arguments: &[OsString]) -> Result<i32, Box<dyn std::error::Error>> {
    #[cfg(not(windows))]
    {
        let _ = (root, arguments);
        eprintln!("Hot Reload 起動は Windows 専用です。");
        Ok(1)
    }
    #[cfg(windows)]
    {
        // dx は --args= を空白で分割するため、空白を含む利用者引数は渡せない。
        if arguments
            .iter()
            .any(|argument| argument.to_string_lossy().chars().any(char::is_whitespace))
        {
            eprintln!("Hot Reload の引数に空白は使用できません。");
            return Ok(1);
        }
        let dx = root.join("target/dx-fixed0710/bin/dx.exe");
        if !dx.is_file() {
            eprintln!("修正版 dx がありません。docs/dev.md の Hot Reload 準備を参照してください。");
            return Ok(1);
        }
        // dx の app 出力は worktree で共有されるため、同時に1つだけ起動する。
        let _lock = open_hot_reload_lock(&root.join("target/hot-reload.lock"))?;
        let job = job::SessionJob::new()?;
        // dx は Desktop だけをビルドし、起動時に app 出力先を削除する。
        // Host は Cargo の安定した出力先へビルドし、session へ複製して渡す。
        let build = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(root)
            .args([
                "build",
                "--locked",
                "--target-dir",
                "target",
                "--target",
                "x86_64-pc-windows-msvc",
                "--profile",
                "hot-reload",
                "--package",
                "host",
                "--bin",
                "lapis-host",
            ])
            .status();
        let built = match build {
            Ok(status) if status.success() => root
                .join("target/x86_64-pc-windows-msvc/hot-reload")
                .join(format!("lapis-host{}", env::consts::EXE_SUFFIX)),
            Ok(status) => {
                eprintln!("Local Host のビルドに失敗したため、Hot Reload を開始しません。");
                return Ok(status.code().unwrap_or(1));
            }
            Err(error) => {
                eprintln!("Local Host のビルドを開始できません: {error}");
                return Ok(1);
            }
        };
        let session = create_session(&root.join("target/dev-sessions"))?;
        let bin = session.join("bin");
        fs::create_dir(&bin)?;
        let host = bin.join(format!("lapis-host{}", env::consts::EXE_SUFFIX));
        fs::copy(&built, &host)?;
        let hosts = session.join("hosts");
        fs::create_dir(&hosts)?;
        // Desktop と同じく、Host を別の Run で起動していれば招待を待ってから接続する。
        wait_for_host_file(root, arguments)?;
        // dx は --args= を空白で分割するため、worktree ルートからの相対パスを使う。
        let state = relative(root, &session.join("state"))?;
        let executable = relative(root, &host)?;
        let mut app_arguments =
            format!("--state-directory {state} --workspace . --local-host-executable {executable}");
        // Desktop と同じ既定にそろえる。利用者が後置した指定が優先される。
        if !arguments
            .iter()
            .any(|argument| argument == "--local-host-directory")
        {
            app_arguments.push_str(" --local-host-directory ");
            app_arguments.push_str(&relative(root, &hosts)?);
        }
        for argument in arguments {
            app_arguments.push(' ');
            app_arguments.push_str(&argument.to_string_lossy());
        }
        eprintln!(
            "Hot Reload session: {}",
            session.file_name().unwrap().to_string_lossy()
        );
        let status = Command::new(&dx)
            .current_dir(root)
            .args([
                "serve",
                "--platform",
                "windows",
                "--target",
                "x86_64-pc-windows-msvc",
                "--hot-patch",
                "--interactive",
                "false",
                "--open",
                "false",
                "--package",
                "lapis",
                "--features",
                "hot-reload",
                "--profile",
                "hot-reload",
            ])
            .arg(format!("--args={app_arguments}"))
            .status();
        job.disarm()?;
        match status {
            Ok(status) => Ok(status.code().unwrap_or(1)),
            Err(error) => {
                eprintln!("修正版 dx を起動できません: {error}");
                Ok(1)
            }
        }
    }
}

#[cfg(windows)]
fn open_hot_reload_lock(path: &Path) -> Result<fs::File, Box<dyn std::error::Error>> {
    use std::os::windows::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
        .map_err(|error| {
            format!("Hot Reload を開始できません。既に起動中か確認してください: {error}").into()
        })
}

#[cfg(windows)]
fn relative(root: &Path, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let path = path
        .strip_prefix(root)
        .map_err(|_| "path is outside the worktree")?;
    Ok(path.to_string_lossy().into_owned())
}

/// `--host` か LAPIS_HOST_FILE が指す招待・profile の出現を待つ。
/// Host を先に起動しなくても繋げるよう、待機は launcher 側に閉じる。
fn wait_for_host_file(
    root: &Path,
    arguments: &[OsString],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut path = None;
    let mut values = arguments.iter();
    while let Some(argument) = values.next() {
        if argument == "--host" {
            path = values.next().map(PathBuf::from);
        }
    }
    let path = match path.or_else(|| env::var_os("LAPIS_HOST_FILE").map(PathBuf::from)) {
        Some(path) if !path.as_os_str().is_empty() => path,
        _ => return Ok(()),
    };
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    if path.exists() {
        return Ok(());
    }
    eprintln!("Host の接続情報を待っています: {}", path.display());
    let deadline = Instant::now() + HOST_FILE_TIMEOUT;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(format!("Host の接続情報が現れません: {}", path.display()).into());
        }
        thread::sleep(HOST_FILE_POLL);
    }
    Ok(())
}

fn create_session(parent: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(parent)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for index in 0..100 {
        let path = parent.join(format!("{}-{timestamp}-{index}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "session directory is unavailable",
    ))
}

fn copy_binary(root: &Path, destination: &Path, name: &str) -> io::Result<PathBuf> {
    let filename = format!("{name}{}", env::consts::EXE_SUFFIX);
    let destination = destination.join(&filename);
    fs::copy(
        root.join("target/dev-build/debug").join(filename),
        &destination,
    )?;
    Ok(destination)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("開発sessionを開始できません: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_flags_are_not_forwarded_to_the_app() {
        let arguments = vec![
            OsString::from("--hot-reload"),
            OsString::from("--host"),
            OsString::from("target/dev-desktop/desktop.json"),
            OsString::from("--host-only"),
        ];
        assert_eq!(
            without_flags(&arguments, &["--hot-reload", "--host-only"]),
            vec![
                OsString::from("--host"),
                OsString::from("target/dev-desktop/desktop.json"),
            ]
        );
    }

    #[test]
    fn launcher_modes_are_exclusive_and_default_to_desktop() {
        let both = vec![
            OsString::from("--hot-reload"),
            OsString::from("--host-only"),
        ];
        assert!(mode(&both).is_err());
        assert_eq!(
            mode(&[OsString::from("--hot-reload")]).unwrap(),
            Mode::HotReload
        );
        assert_eq!(
            mode(&[OsString::from("--host-only")]).unwrap(),
            Mode::HostOnly
        );
        assert_eq!(mode(&[]).unwrap(), Mode::Desktop);
    }

    #[test]
    fn sessions_never_reuse_host_or_binary_directories() {
        let base = create_session(&env::temp_dir().join("lapis-dev-session-tests")).unwrap();
        let first = create_session(&base).unwrap();
        let second = create_session(&base).unwrap();
        assert_ne!(first, second);
        assert_ne!(first.join("hosts"), second.join("hosts"));
        assert_ne!(first.join("bin"), second.join("bin"));
        fs::remove_dir(first).unwrap();
        fs::remove_dir(second).unwrap();
        fs::remove_dir(base).unwrap();
    }
}

#[cfg(windows)]
mod job {
    use std::{io, mem, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::GetCurrentProcess,
        },
    };

    pub struct SessionJob(HANDLE);
    impl SessionJob {
        pub fn new() -> io::Result<Self> {
            // Jobはprocess実体を保持するため、PIDの再利用に依存しない。
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            job.set_kill_on_close(true)?;
            if unsafe { AssignProcessToJobObject(job.0, GetCurrentProcess()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
        fn set_kill_on_close(&self, enabled: bool) -> io::Result<()> {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
            if enabled {
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            if unsafe {
                SetInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    mem::size_of_val(&info) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        pub fn disarm(self) -> io::Result<()> {
            self.set_kill_on_close(false)
        }
    }
    impl Drop for SessionJob {
        fn drop(&mut self) {
            // 通常終了やエラーではHostを維持し、強制停止時はOSがJobを閉じる。
            let _ = self.set_kill_on_close(false);
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
