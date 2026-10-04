//! Bounded child output, including a child that never exits. No line buffering:
//! a malformed or very long log line must not allocate unbounded memory.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const LOG_BYTES: u64 = 1024 * 1024;

pub(super) struct LoggedChild {
    pub(super) process: Child,
    drains: Vec<JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
    log: Arc<Mutex<RotatingLog>>,
}

impl LoggedChild {
    pub(super) fn spawn(command: &mut Command, dir: &Path) -> anyhow::Result<Self> {
        let log = Arc::new(Mutex::new(RotatingLog::open(dir)?));
        let mut process = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        #[cfg(unix)]
        if let Err(error) = nonblocking(process.stdout.as_ref().expect("piped stdout"))
            .and_then(|()| nonblocking(process.stderr.as_ref().expect("piped stderr")))
        {
            let _ = process.kill();
            let _ = process.wait();
            return Err(error.into());
        }
        let streams: [Box<dyn Read + Send>; 2] = [
            Box::new(process.stdout.take().expect("piped stdout")),
            Box::new(process.stderr.take().expect("piped stderr")),
        ];
        let stopped = Arc::new(AtomicBool::new(false));
        let mut child = Self {
            process,
            drains: Vec::new(),
            stopped: stopped.clone(),
            log: log.clone(),
        };
        for mut stream in streams {
            let log = log.clone();
            let stopped = stopped.clone();
            match thread::Builder::new()
                .name("cloudflared-log".into())
                .spawn(move || {
                    let mut bytes = [0; 8192];
                    loop {
                        if stopped.load(Ordering::Acquire) {
                            return;
                        }
                        let count = match stream.read(&mut bytes) {
                            Ok(0) => return,
                            Ok(count) => count,
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                continue
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(10));
                                continue;
                            }
                            Err(_) => return,
                        };
                        let mut log = log.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        // Keep draining on a full or unavailable disk, so logging
                        // cannot deadlock the connector. A new child retries storage.
                        if !log.failed {
                            if let Err(error) = log.write(&bytes[..count]) {
                                log.failed = true;
                                log::warn!(target: "sys", "cloudflared log unavailable: {error}");
                            }
                        }
                    }
                }) {
                Ok(drain) => child.drains.push(drain),
                Err(error) => {
                    child.terminate();
                    return Err(error.into());
                }
            }
        }
        Ok(child)
    }

    /// A wrapper's descendant may retain inherited pipes after the direct child
    /// exits. Allow a bounded final drain, then close the writer before returning
    /// so no old reader can race a new child or recreate reset data.
    pub(super) fn finish(self) {
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.drains.iter().any(|drain| !drain.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        self.stopped.store(true, Ordering::Release);
        {
            let mut log = self
                .log
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            log.failed = true;
            log.file.take();
        }
        for drain in self.drains {
            // Unix pipes are nonblocking, so cancellation ends their readers.
            // On other platforms, an inherited blocking reader may outlive the
            // child; its closed writer cannot modify current or reset data.
            if cfg!(unix) || drain.is_finished() {
                let _ = drain.join();
            }
        }
    }

    pub(super) fn terminate(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        self.finish();
    }
}

#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: the owned child pipe stays alive throughout both calls; fcntl
    // changes only its status flags and does not transfer or close ownership.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

struct RotatingLog {
    path: PathBuf,
    previous: PathBuf,
    file: Option<File>,
    bytes: u64,
    failed: bool,
}

impl RotatingLog {
    fn open(dir: &Path) -> std::io::Result<Self> {
        let path = dir.join("cloudflared.log");
        let previous = dir.join("cloudflared.log.1");
        if previous.exists() {
            bounded_existing_file(&previous)?;
        }
        let file = bounded_existing_file(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            path,
            previous,
            file: Some(file),
            bytes,
            failed: false,
        })
    }

    fn write(&mut self, mut bytes: &[u8]) -> std::io::Result<()> {
        while !bytes.is_empty() {
            if self.bytes == LOG_BYTES {
                self.file.take();
                match std::fs::remove_file(&self.previous) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                std::fs::rename(&self.path, &self.previous)?;
                self.file = Some(bounded_existing_file(&self.path)?);
                self.bytes = 0;
            }
            let count = bytes.len().min((LOG_BYTES - self.bytes) as usize);
            self.file
                .as_mut()
                .expect("open log")
                .write_all(&bytes[..count])?;
            self.bytes += count as u64;
            bytes = &bytes[count..];
        }
        Ok(())
    }
}

fn bounded_existing_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let len = file.metadata()?.len();
    if len > LOG_BYTES {
        // Upgrade from the previous unbounded writer, keeping only its tail.
        file.seek(SeekFrom::Start(len - LOG_BYTES))?;
        let mut tail = vec![0; LOG_BYTES as usize];
        file.read_exact(&mut tail)?;
        file.set_len(0)?;
        file.write_all(&tail)?;
    }
    Ok(file)
}
