use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{SystemTime, UNIX_EPOCH},
};

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("worktree root is unavailable")?;
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
    let arguments: Vec<_> = env::args_os().skip(1).collect();
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
        command.args(&arguments).status()?
    };
    #[cfg(windows)]
    job.disarm()?;
    Ok(status.code().unwrap_or(1))
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
