//! Linux-only subprocess boundary. Never fall back to an unrestricted process.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub(crate) const MAX_INPUT: usize = 1_048_576;
pub(crate) const MAX_ARTIFACT: u64 = 16 * 1_048_576;

pub(crate) struct Workspace {
    pub path: PathBuf,
}
impl Workspace {
    pub fn new() -> io::Result<Self> {
        #[cfg(not(target_os = "linux"))]
        return Err(io::Error::other("isolated forge requires Linux"));
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::DirBuilderExt;
            // Fixed system temp parent: do not honor a service's shared TMPDIR.
            for _ in 0..8 {
                let mut entropy = [0u8; 16];
                File::open("/dev/urandom")?.read_exact(&mut entropy)?;
                let name: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
                let path = PathBuf::from("/tmp").join(format!("sbh-forge-{name}"));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Ok(Self { path }),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            }
            Err(io::Error::other("could not allocate private workspace"))
        }
    }
    pub fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(self.path.join(name))?.write_all(bytes)
    }
    pub fn cleanup(&self) -> io::Result<()> {
        fs::remove_dir_all(&self.path)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Supported distro library layout; never mount the whole /usr/lib or /lib tree.
/// Both executable loaders and their multiarch dependencies must be present.
fn bind_system_libraries(command: &mut Command) -> io::Result<()> {
    let (multiarch, loader) = match std::env::consts::ARCH {
        "x86_64" => ("x86_64-linux-gnu", "/lib64/ld-linux-x86-64.so.2"),
        "aarch64" => ("aarch64-linux-gnu", "/lib/ld-linux-aarch64.so.1"),
        _ => return Err(io::Error::other("unsupported forge system-library layout")),
    };
    let source = PathBuf::from("/usr/lib").join(multiarch).canonicalize()?;
    if !source.is_dir() {
        return Err(io::Error::other("missing multiarch library directory"));
    }
    let destination = PathBuf::from("/usr/lib").join(multiarch);
    let loader_source = Path::new(loader).canonicalize()?;
    if !loader_source.is_file() {
        return Err(io::Error::other("missing dynamic loader"));
    }
    command
        .arg("--ro-bind")
        .arg(source)
        .arg(destination)
        .args(["--symlink", "usr/lib", "/lib"])
        .arg("--ro-bind")
        .arg(loader_source)
        .arg(loader);
    Ok(())
}

/// Read-only compiler toolchain, disposable workdir and otherwise empty root.
/// Bubblewrap/prlimit must be installed by the operator; absence fails closed.
pub(crate) fn compiler_command(sysroot: &Path, work: &Path) -> io::Result<Command> {
    if !cfg!(target_os = "linux") {
        return Err(io::Error::other("isolated compilation requires Linux"));
    }
    let mut command = limited_command(2 * 1024 * 1024 * 1024, 60);
    command.arg("/usr/bin/bwrap").args([
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--clearenv",
        "--setenv",
        "PATH",
        "/toolchain/bin",
        "--setenv",
        "HOME",
        "/nonexistent",
        "--setenv",
        "TMPDIR",
        "/tmp",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]);
    bind_system_libraries(&mut command)?;
    command
        .arg("--ro-bind")
        .arg(sysroot)
        .arg("/toolchain")
        .arg("--bind")
        .arg(work)
        .arg("/work")
        .args(["--chdir", "/work", "--", "/toolchain/bin/rustc"]);
    Ok(command)
}

pub(crate) fn limited_command(address_space: u64, cpu_seconds: u64) -> Command {
    let mut command = Command::new("/usr/bin/prlimit");
    command.env_clear().env("PATH", "/usr/bin:/bin").args([
        format!("--as={address_space}"),
        format!("--cpu={cpu_seconds}"),
        format!("--fsize={MAX_ARTIFACT}"),
        "--nofile=128".into(),
        "--core=0".into(),
        "--".into(),
    ]);
    command
}

#[cfg(unix)]
fn nonblocking(fd: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: this is a live owned pipe descriptor. F_GETFL takes no third
    // argument; F_SETFL changes only its status flags, retaining all other flags.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn drain(input: &mut impl Read, buffer: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    let mut block = [0u8; 4096];
    loop {
        match input.read(&mut block) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                let room = limit - buffer.len();
                if n > room {
                    return Err(io::Error::other("subprocess output limit exceeded"));
                }
                buffer.extend_from_slice(&block[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(target_os = "linux")]
struct ProcessGuard {
    child: std::process::Child,
    reaped: bool,
}
#[cfg(target_os = "linux")]
impl ProcessGuard {
    fn exited(&self) -> io::Result<bool> {
        // SAFETY: siginfo_t is a C output structure valid when zero-initialized;
        // waitid writes within it. WNOWAIT keeps the child PID allocated until
        // group termination, preventing a recycled PID from naming another group.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful waitid initializes si_pid, or leaves the zero PID
        // when WNOHANG finds no terminated child.
        Ok(unsafe { info.si_pid() } != 0)
    }
    fn terminate_group(&self) {
        // SAFETY: the unreaped positive child PID still owns this process group.
        let group = self.child.id() as libc::pid_t;
        if group > 1 {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
    }
    fn finish(&mut self) -> io::Result<std::process::ExitStatus> {
        self.terminate_group();
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}
#[cfg(target_os = "linux")]
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if !self.reaped {
            self.terminate_group();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Nonblocking I/O keeps stdin, output EOF and process exit under one deadline.
/// No worker thread can outlive a call because a descendant retains a pipe.
/// Process groups are killed/reaped on every exit; compiler PID namespaces add
/// containment for descendants that create a new session inside the sandbox.
pub(crate) fn run(
    command: &mut Command,
    input: &[u8],
    deadline: Duration,
    max_output: usize,
) -> io::Result<Output> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (command, input, deadline, max_output);
        Err(io::Error::other("unsupported process boundary"))
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        if input.len() > MAX_INPUT {
            return Err(io::Error::other("input limit exceeded"));
        }
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let start = Instant::now();
        let mut child = ProcessGuard {
            child: command.spawn()?,
            reaped: false,
        };
        let mut stdin = child.child.stdin.take();
        let mut stdout = child.child.stdout.take().expect("piped stdout");
        let mut stderr = child.child.stderr.take().expect("piped stderr");
        nonblocking(stdin.as_ref().expect("piped stdin"))?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let mut written = 0;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_eof = false;
        let mut err_eof = false;
        loop {
            if start.elapsed() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "subprocess deadline exceeded",
                ));
            }
            if let Some(pipe) = stdin.as_mut() {
                if written == input.len() {
                    stdin.take();
                } else {
                    let end = (written + 16384).min(input.len());
                    match pipe.write(&input[written..end]) {
                        Ok(0) => {
                            return Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "subprocess stdin closed",
                            ))
                        }
                        Ok(n) => written += n,
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) => {}
                        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                            stdin.take();
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            if !out_eof {
                out_eof = drain(&mut stdout, &mut out, max_output)?;
            }
            if !err_eof {
                err_eof = drain(&mut stderr, &mut err, max_output)?;
            }
            if child.exited()? && out_eof && err_eof {
                let status = child.finish()?;
                if status.success() && written != input.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "could not deliver complete subprocess input",
                    ));
                }
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn compiler_library_mounts_exclude_broad_host_trees() {
        let cmd =
            compiler_command(Path::new("/trusted/toolchain"), Path::new("/private/work")).unwrap();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        for mount in args.windows(3).filter(|w| w[0] == "--ro-bind") {
            assert!(!["/usr/lib", "/lib", "/lib64"].contains(&mount[1]));
            assert!(!["/usr/lib", "/lib", "/lib64"].contains(&mount[2]));
        }
        assert!(args.iter().any(|a| a.contains("linux-gnu")));
    }

    #[test]
    fn output_is_limited_before_accumulation() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "while :; do printf '01234567890123456789'; done"]);
        let e = run(&mut cmd, b"", Duration::from_secs(2), 1024).unwrap_err();
        assert!(e.to_string().contains("output limit"));
    }
    #[test]
    fn blocked_stdin_and_descendants_share_deadline() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 20 & wait"]);
        let start = Instant::now();
        let e = run(
            &mut cmd,
            &vec![b'x'; MAX_INPUT],
            Duration::from_millis(100),
            1024,
        )
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn detached_pipe_holder_cannot_extend_deadline_after_leader_exit() {
        let mut cmd = Command::new("/bin/sh");
        // The detached process is self-terminating, even on test assertion failure.
        cmd.args([
            "-c",
            "setsid /bin/sh -c 'sleep 2; printf delayed' & sleep 0.1; exit 0",
        ]);
        let start = Instant::now();
        let error = run(&mut cmd, b"", Duration::from_millis(200), 1024).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn private_workspace_rejects_existing_files_and_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = Workspace::new().unwrap();
        assert_eq!(
            fs::metadata(&dir.path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        dir.write("file", b"first").unwrap();
        assert!(dir.write("file", b"second").is_err());
        symlink("file", dir.path.join("link")).unwrap();
        assert!(dir.write("link", b"second").is_err());
        assert_eq!(fs::read(dir.path.join("file")).unwrap(), b"first");
        dir.cleanup().unwrap();
        assert!(dir.cleanup().is_err());
    }
    #[test]
    #[ignore = "permission-denial fixture requires an unprivileged Linux test user"]
    fn cleanup_permission_denial_is_observable_and_recoverable() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no pointer arguments or side effects.
        assert_ne!(
            unsafe { libc::geteuid() },
            0,
            "run this fixture as an unprivileged user"
        );
        let workspace = Workspace::new().unwrap();
        let nested = workspace.path.join("locked");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("fixture"), b"synthetic").unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = workspace.cleanup();
        // Restore before assertions so even an assertion failure leaves no locked artifact.
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(denied.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        workspace.cleanup().unwrap();
        assert!(!workspace.path.exists());
    }

    #[test]
    #[ignore = "requires operator-installed bubblewrap/prlimit and enabled user namespaces; run explicitly"]
    fn sandbox_is_empty_and_does_not_inherit_host_markers() {
        let sysroot = Command::new("rustc")
            .args(["--print", "sysroot"])
            .output()
            .unwrap();
        assert!(sysroot.status.success());
        let root = PathBuf::from(String::from_utf8(sysroot.stdout).unwrap().trim());
        let dir = Workspace::new().unwrap();
        let marker = Workspace::new().unwrap();
        marker.write("canary", b"SYNTHETIC_SECRET").unwrap();
        // No global environment mutation: the compiler command explicitly clears it.
        dir.write("tool.rs", b"pub fn run() {} ").unwrap();
        let mut clean = compiler_command(&root, &dir.path).unwrap();
        clean
            .env("SGAIL_COMPILER_CANARY", "SYNTHETIC_SECRET")
            .args([
                "--crate-type=lib",
                "--emit=metadata",
                "/work/tool.rs",
                "-o",
                "/work/tool.rmeta",
            ]);
        let baseline = run(&mut clean, b"", Duration::from_secs(15), 8192).unwrap();
        assert!(
            baseline.status.success(),
            "sandbox baseline failed (no unrestricted fallback): {}",
            String::from_utf8_lossy(&baseline.stderr)
        );
        assert!(
            Path::new("/usr/lib/os-release").is_file(),
            "host fixture must exist"
        );
        fs::write(
            dir.path.join("tool.rs"),
            b"pub const X: &str = include_str!(\"/usr/lib/os-release\");",
        )
        .unwrap();
        let mut excluded = compiler_command(&root, &dir.path).unwrap();
        excluded.args([
            "--crate-type=lib",
            "--emit=metadata",
            "/work/tool.rs",
            "-o",
            "/work/hidden.rmeta",
        ]);
        let result = run(&mut excluded, b"", Duration::from_secs(15), 8192).unwrap();
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(
            error.contains("couldn't read") && error.contains("/usr/lib/os-release"),
            "{error}"
        );
        for source in [
            "pub const X: &str = env!(\"SGAIL_COMPILER_CANARY\");".to_string(),
            format!(
                "pub const X: &str = include_str!({:?});",
                marker.path.join("canary")
            ),
        ] {
            fs::write(dir.path.join("tool.rs"), source).unwrap();
            let mut cmd = compiler_command(&root, &dir.path).unwrap();
            cmd.env("SGAIL_COMPILER_CANARY", "SYNTHETIC_SECRET").args([
                "--crate-type=lib",
                "--emit=metadata",
                "/work/tool.rs",
                "-o",
                "/work/tool.rmeta",
            ]);
            let result = run(&mut cmd, b"", Duration::from_secs(15), 8192).unwrap();
            assert!(!result.status.success());
        }
    }
}
