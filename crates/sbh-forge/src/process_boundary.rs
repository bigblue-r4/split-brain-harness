//! Linux-only subprocess boundary. Never fall back to an unrestricted process.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
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
    // Dynamic loader and system libraries only, not /usr, /etc or user homes.
    for path in ["/usr/lib", "/lib", "/lib64"] {
        if Path::new(path).exists() {
            command.args(["--ro-bind", path, path]);
        }
    }
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

fn reader(mut input: impl Read, max: usize, overflow: Arc<AtomicBool>) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(max.min(8192));
    let mut block = [0u8; 4096];
    loop {
        let n = input.read(&mut block)?;
        if n == 0 {
            return Ok(buf);
        }
        let room = max - buf.len();
        buf.extend_from_slice(&block[..n.min(room)]);
        if n > room {
            overflow.store(true, Ordering::Release);
            return Ok(buf);
        }
    }
}

/// Entire stdin/stdout/stderr lifecycle shares one deadline. Output is bounded
/// while reading, and every exit path terminates the process group and reaps it.
pub(crate) fn run(
    command: &mut Command,
    input: &[u8],
    deadline: Duration,
    max_output: usize,
) -> io::Result<Output> {
    #[cfg(not(unix))]
    {
        let _ = (command, input, deadline, max_output);
        Err(io::Error::other("unsupported process boundary"))
    }
    #[cfg(unix)]
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
        let mut child = command.spawn()?;
        let group = child.id();
        let overflow = Arc::new(AtomicBool::new(false));
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let out_limit = overflow.clone();
        let err_limit = overflow.clone();
        let out = std::thread::spawn(move || reader(stdout, max_output, out_limit));
        let err = std::thread::spawn(move || reader(stderr, max_output, err_limit));
        let mut stdin = child.stdin.take().expect("piped stdin");
        let bytes = input.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&bytes));
        let start = Instant::now();
        let result = loop {
            if overflow.load(Ordering::Acquire) {
                break Err(io::Error::other("subprocess output limit exceeded"));
            }
            if start.elapsed() >= deadline {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "subprocess deadline exceeded",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => break Err(e),
            }
        };
        // Also kill descendants when the leader exits before closing its pipes.
        let killed = Command::new("/bin/kill")
            .env_clear()
            .args(["-KILL", "--", &format!("-{group}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if killed.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        let write_result = writer
            .join()
            .map_err(|_| io::Error::other("stdin worker failed"))?;
        let stdout = out
            .join()
            .map_err(|_| io::Error::other("stdout worker failed"))??;
        let stderr = err
            .join()
            .map_err(|_| io::Error::other("stderr worker failed"))??;
        let status = result?;
        if overflow.load(Ordering::Acquire) {
            return Err(io::Error::other("subprocess output limit exceeded"));
        }
        if status.success() {
            write_result?;
        }
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
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
