use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{SystemTime, UNIX_EPOCH},
};

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("worktree root is unavailable")?;
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    // Hot Reload は dx が Desktop をビルドするため、ide-build を実行しない。
    if arguments.first().is_some_and(|arg| arg == "--hot-reload") {
        return hot_reload(root, &arguments[1..]);
    }
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let build = Command::new(cargo)
        .current_dir(root)
        .arg("ide-build")
        .status()?;
    if !build.success() {
        return Ok(build.code().unwrap_or(1));
    }

    #[cfg(windows)]
    let job = job::SessionJob::new()?;
    let session = create_session(&root.join("target/dev-sessions"))?;
    let bin = session.join("bin");
    fs::create_dir(&bin)?;
    let desktop = copy_binary(root, &bin, "lapis")?;
    copy_binary(root, &bin, "lapis-host")?;
    let host_only = arguments.first().is_some_and(|arg| arg == "--host-only");
    let status = if host_only {
        let mut host = Command::new(bin.join(format!("lapis-host{}", env::consts::EXE_SUFFIX)));
        host.current_dir(root)
            .arg("--workspace")
            .arg(root)
            .arg("--certificate")
            .arg(session.join("host.der"))
            .args(&arguments[1..]);
        eprintln!(
            "Host session: {}",
            session.file_name().unwrap().to_string_lossy()
        );
        host.status()?
    } else {
        let hosts = session.join("hosts");
        fs::create_dir(&hosts)?;
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

fn hot_reload(root: &Path, arguments: &[OsString]) -> Result<i32, Box<dyn std::error::Error>> {
    #[cfg(not(windows))]
    {
        let _ = (root, arguments);
        eprintln!("Hot Reload 起動は Windows 専用です。");
        Ok(1)
    }
    #[cfg(windows)]
    {
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
        // dx は --args= を空白で分割するため、worktree ルートからの相対パスを使う。
        let state = relative(root, &session.join("state"))?;
        let executable = relative(root, &host)?;
        let mut app_arguments =
            format!("--state-directory {state} --workspace . --local-host-executable {executable}");
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
