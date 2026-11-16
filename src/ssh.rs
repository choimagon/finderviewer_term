use crate::fs::{Entry, Sort};
use anyhow::{bail, Context, Result};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant, UNIX_EPOCH},
};
use tempfile::TempDir;

const AGENT: &str = include_str!("ssh_agent.py");

#[derive(Debug)]
pub struct Listing {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}
impl Listing {
    pub fn sorted(&self, hidden: bool, sort: Sort) -> Vec<Entry> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|e| hidden || !e.name.starts_with('.'))
            .cloned()
            .collect();
        entries.sort_by(|a, b| {
            let names = || {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then(a.name.cmp(&b.name))
            };
            match sort {
                Sort::Folders => b.is_dir.cmp(&a.is_dir).then_with(names),
                Sort::Name => names(),
                Sort::Recent => b.modified.cmp(&a.modified).then_with(names),
            }
        });
        entries
    }
}

pub fn validate_target(target: &str) -> Result<()> {
    if target.is_empty()
        || target.starts_with('-')
        || target.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        bail!("--ssh 뒤에 SSH 호스트 별칭 또는 사용자@서버를 지정하세요.");
    }
    Ok(())
}

#[derive(Debug)]
struct Session {
    target: String,
    socket: PathBuf,
    _directory: TempDir,
}
impl Session {
    fn command(&self, interactive: bool) -> Command {
        let mut command = Command::new("ssh");
        command.args(["-T", "-S"]).arg(&self.socket).args([
            "-oControlMaster=auto",
            "-oControlPersist=600",
            "-oConnectTimeout=10",
            "-oServerAliveInterval=10",
            "-oServerAliveCountMax=2",
        ]);
        if !interactive {
            command.arg("-oBatchMode=yes");
        }
        command.arg("--").arg(&self.target).stdin(Stdio::null());
        command
    }
    fn close(&self) {
        // The control command does not connect to the server if the socket is gone.
        let child = Command::new("ssh")
            .arg("-S")
            .arg(&self.socket)
            .args(["-O", "exit", "--"])
            .arg(&self.target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if start.elapsed() < Duration::from_secs(1) => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Clone, Debug)]
pub struct Remote(Arc<Session>);
impl Remote {
    pub fn connect(target: String, path: &Path) -> Result<(Self, Listing)> {
        validate_target(&target)?;
        // Keep ControlPath short enough for Unix-domain sockets, including on macOS.
        let mut builder = tempfile::Builder::new();
        builder.prefix("fvssh-");
        #[cfg(unix)]
        let directory = builder.tempdir_in("/tmp")?;
        #[cfg(not(unix))]
        let directory = builder.tempdir()?;
        let remote = Self(Arc::new(Session {
            target,
            socket: directory.path().join("s"),
            _directory: directory,
        }));
        // Authenticate before entering raw terminal mode. OpenSSH handles passwords,
        // host keys, ssh-agent, and ~/.ssh/config with its usual /dev/tty prompts.
        let mut command = remote.request("list", path, true)?;
        command.stderr(Stdio::inherit());
        let data = execute(
            command,
            None,
            Duration::from_secs(180),
            &AtomicBool::new(false),
        )?;
        let listing = parse_listing(&data)?;
        Ok((remote, listing))
    }
    pub fn target(&self) -> &str {
        &self.0.target
    }
    pub fn close(&self) {
        self.0.close();
    }
    fn request(&self, operation: &str, path: &Path, interactive: bool) -> Result<Command> {
        let path = path.to_str().context("SSH 경로는 UTF-8이어야 합니다.")?;
        if path.contains('\0') {
            bail!("경로에 NUL 문자를 사용할 수 없습니다.");
        }
        let script = format!(
            "python3 -c {} {} {}",
            shell_words::quote(AGENT),
            shell_words::quote(operation),
            shell_words::quote(path)
        );
        let mut command = self.0.command(interactive);
        command.arg(script);
        Ok(command)
    }
    pub fn list(&self, path: &Path, cancel: &AtomicBool) -> Result<Listing> {
        let data = execute(
            self.request("list", path, false)?,
            None,
            Duration::from_secs(30),
            cancel,
        )?;
        parse_listing(&data)
    }
    pub fn download(&self, path: &Path, cancel: &AtomicBool) -> Result<PathBuf> {
        let filename = path.file_name().context("파일 이름이 없습니다.")?;
        let directory = tempfile::Builder::new()
            .prefix("finderviewer-ssh-")
            .tempdir()?;
        let local = directory.path().join(filename);
        let staged = tempfile::NamedTempFile::new()?;
        let file = staged.reopen()?;
        execute(
            self.request("read", path, false)?,
            Some(file),
            Duration::from_secs(300),
            cancel,
        )?;
        finish_download(staged.path(), &local)?;
        if cancel.load(Ordering::Relaxed) {
            bail!("SSH 작업을 취소했습니다.");
        }
        // Each opening gets a separate copy. Keep it after exit for the local app.
        let _ = directory.keep();
        Ok(local)
    }
}

fn finish_download(staged: &Path, local: &Path) -> Result<()> {
    let mut source = File::open(staged)?;
    let mut header = [0u8; 16];
    source
        .read_exact(&mut header)
        .context("SSH 파일 헤더가 누락됐습니다.")?;
    if &header[..8] != b"FVFILE1\0" {
        bail!("SSH 파일 형식이 올바르지 않습니다. 비대화형 셸 출력을 확인하세요.");
    }
    let expected = u64::from_be_bytes(header[8..].try_into()?);
    if source.metadata()?.len().checked_sub(16) != Some(expected) {
        bail!("전송 도중 원격 파일 크기가 변경됐거나 파일 전송이 불완전합니다. 다시 여세요.");
    }
    let mut destination = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(local)?;
    std::io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    Ok(())
}

// Drain both pipes while waiting so large listings cannot fill the SSH pipe.
// Every operation has a deadline and can be cancelled from the UI.
fn execute(
    mut command: Command,
    output: Option<File>,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    command.stdout(output.map(Stdio::from).unwrap_or_else(Stdio::piped));
    // Initial authentication keeps stderr attached to the terminal. Other requests
    // capture it so errors do not corrupt the full-screen display.
    let interactive = !command.get_args().any(|a| a == "-oBatchMode=yes");
    if !interactive {
        command.stderr(Stdio::piped());
    }
    let mut child = command.spawn().context("ssh 명령을 실행하지 못했습니다.")?;
    let stdout = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let start = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            if cancel.load(Ordering::Relaxed) {
                bail!("SSH 작업을 취소했습니다.");
            }
            bail!("SSH 응답 시간이 초과됐습니다. 연결을 확인한 뒤 F5로 다시 시도하세요.");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let data = stdout
        .map(|h| -> Result<Vec<u8>> {
            Ok(h.join()
                .map_err(|_| anyhow::anyhow!("SSH 출력 읽기 실패"))??)
        })
        .transpose()?
        .unwrap_or_default();
    let error = stderr
        .map(|h| -> Result<Vec<u8>> {
            Ok(h.join()
                .map_err(|_| anyhow::anyhow!("SSH 오류 읽기 실패"))??)
        })
        .transpose()?
        .unwrap_or_default();
    if !status.success() {
        bail!(
            "SSH 요청 실패 ({status}): {}",
            String::from_utf8_lossy(&error).trim()
        );
    }
    Ok(data)
}

fn parse_listing(data: &[u8]) -> Result<Listing> {
    let text = std::str::from_utf8(data).context("SSH 파일 목록이 UTF-8이 아닙니다.")?;
    let mut fields = text.split_terminator('\0');
    if fields.next() != Some("FVSSH1") || !text.ends_with('\0') {
        bail!("SSH 파일 목록 형식이 올바르지 않습니다. 서버의 Python 3과 비대화형 셸 출력을 확인하세요.");
    }
    let path = PathBuf::from(fields.next().context("원격 폴더 경로 누락")?);
    if !path.is_absolute() {
        bail!("원격 폴더의 절대 경로가 필요합니다.");
    }
    let mut entries = Vec::new();
    while let Some(name) = fields.next() {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            bail!("잘못된 원격 파일 이름입니다.");
        }
        let mut next = || fields.next().context("원격 파일 정보 누락");
        let is_dir = next()?.parse::<u8>()? == 1;
        let size = next()?.parse()?;
        let nanos = next()?.parse::<u64>()?;
        let is_symlink = next()?.parse::<u8>()? == 1;
        entries.push(Entry {
            path: path.join(name),
            name: name.into(),
            is_dir,
            size,
            modified: UNIX_EPOCH
                .checked_add(Duration::from_nanos(nanos))
                .context("잘못된 수정 시간")?,
            is_symlink,
        });
    }
    Ok(Listing { path, entries })
}

pub struct Task<T> {
    receiver: mpsc::Receiver<Result<T>>,
    cancel: Arc<AtomicBool>,
}
impl<T: Send + 'static> Task<T> {
    pub fn spawn(job: impl FnOnce(&AtomicBool) -> Result<T> + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            let _ = sender.send(job(&flag));
        });
        Self { receiver, cancel }
    }
    pub fn poll(&self) -> Option<Result<T>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err(anyhow::anyhow!("SSH 작업이 중단됐습니다.")))
            }
        }
    }
}
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_listing_preserves_unicode_spaces_and_newlines() {
        let data = "FVSSH1\0/home/a b\0한글\n파일.md\x000\x0012\x001000000000\x000\0folder\x001\x000\x002000000000\x000\0";
        let listing = parse_listing(data.as_bytes()).unwrap();
        assert_eq!(
            listing.entries[0].path,
            Path::new("/home/a b/한글\n파일.md")
        );
        assert_eq!(listing.sorted(true, Sort::Folders)[0].name, "folder");
        assert_eq!(listing.sorted(true, Sort::Recent)[0].name, "folder");
        for bad in [
            "FVSSH1\0/remote\0../escape\x000\x000\x000\x000\0",
            "banner\nFVSSH1\0/remote\0",
            "FVSSH1\0/remote\0partial\0",
        ] {
            assert!(parse_listing(bad.as_bytes()).is_err());
        }
    }
    #[test]
    fn target_cannot_inject_ssh_options() {
        for target in [
            "",
            "-oProxyCommand=bad",
            "host\ncommand",
            "user@host command",
        ] {
            assert!(validate_target(target).is_err());
        }
        for target in ["user@192.0.2.10", "my-server", "user@::1"] {
            validate_target(target).unwrap();
        }
    }
    #[test]
    fn agent_lists_and_reads_literal_shell_characters() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory
            .path()
            .join("한글 ' $(touch should-not-exist)\n.md");
        std::fs::write(&path, [0, 255, 1, 2]).unwrap();
        let output = Command::new("python3")
            .args(["-c", AGENT, "list"])
            .arg(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let listing = parse_listing(&output.stdout).unwrap();
        assert_eq!(
            listing.entries[0].name,
            path.file_name().unwrap().to_str().unwrap()
        );
        let script = format!(
            "python3 -c {} read {}",
            shell_words::quote(AGENT),
            shell_words::quote(path.to_str().unwrap())
        );
        let output = Command::new("sh").args(["-c", &script]).output().unwrap();
        assert!(output.status.success());
        assert_eq!(&output.stdout[..8], b"FVFILE1\0");
        let stage = directory.path().join("stage");
        let downloaded = directory.path().join("downloaded");
        std::fs::write(&stage, &output.stdout).unwrap();
        finish_download(&stage, &downloaded).unwrap();
        assert_eq!(std::fs::read(downloaded).unwrap(), [0, 255, 1, 2]);
        std::fs::write(&stage, b"banner corrupts binary\0").unwrap();
        assert!(finish_download(&stage, &directory.path().join("bad")).is_err());
    }
    #[test]
    fn command_timeout_and_cancellation_are_bounded() {
        let mut command = Command::new("python3");
        command.args(["-c", "import time\ntime.sleep(10)"]);
        assert!(execute(
            command,
            None,
            Duration::from_millis(80),
            &AtomicBool::new(false)
        )
        .is_err());
        let mut command = Command::new("python3");
        command.args(["-c", "import time\ntime.sleep(10)"]);
        assert!(execute(
            command,
            None,
            Duration::from_secs(5),
            &AtomicBool::new(true)
        )
        .is_err());
    }

    #[test]
    #[ignore = "requires FV_TEST_SSH_TARGET, FV_TEST_SSH_ROOT, FV_TEST_SSH_FILES"]
    fn live_ssh_list_and_download() {
        let target = std::env::var("FV_TEST_SSH_TARGET").unwrap();
        let root = PathBuf::from(std::env::var("FV_TEST_SSH_ROOT").unwrap());
        let (remote, listing) = Remote::connect(target, &root).unwrap();
        assert!(!listing.entries.is_empty());
        for requested in std::env::var("FV_TEST_SSH_FILES").unwrap().lines() {
            let requested = Path::new(requested);
            let listing = remote
                .list(requested.parent().unwrap(), &AtomicBool::new(false))
                .unwrap();
            let entry = listing
                .entries
                .iter()
                .find(|e| e.name == requested.file_name().unwrap().to_str().unwrap())
                .unwrap();
            let first = remote.download(requested, &AtomicBool::new(false)).unwrap();
            let second = remote.download(requested, &AtomicBool::new(false)).unwrap();
            assert_ne!(first, second);
            assert_eq!(std::fs::metadata(&first).unwrap().len(), entry.size);
            assert_eq!(
                std::fs::read(&first).unwrap(),
                std::fs::read(&second).unwrap()
            );
            assert_eq!(first.file_name(), requested.file_name());
            std::fs::remove_dir_all(first.parent().unwrap()).unwrap();
            std::fs::remove_dir_all(second.parent().unwrap()).unwrap();
        }
        // Shell metacharacters must be a literal nonexistent path, never a command.
        assert!(remote
            .list(
                Path::new("/nonexistent ' $(touch /tmp/fvssh-injected)"),
                &AtomicBool::new(false)
            )
            .is_err());
        remote.close();
        assert!(!remote.0.socket.exists());
    }
}
