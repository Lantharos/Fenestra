use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::process::{Command, Stdio};

const LOCK_TIMEOUT: Duration = Duration::from_secs(600);
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

pub(crate) struct HostBuildLock {
    path: PathBuf,
}

impl HostBuildLock {
    pub(crate) fn acquire(runtime_dir: &Path) -> Result<Self, String> {
        let path = runtime_dir.join(".sabine-host-build.lock");
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    let _ = writeln!(file, "started={}", unix_timestamp_secs());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err(format!(
                            "timed out waiting for Sabine CEF host build lock at {}",
                            path.display()
                        ));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

impl Drop for HostBuildLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path) -> bool {
    if let Some(pid) = lock_holder_pid(path) {
        return !process_alive(pid);
    }
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= LOCK_STALE_AFTER)
}

fn lock_holder_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("pid=")
                .and_then(|value| value.trim().parse().ok())
        })
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    return Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::{CloseHandle, STILL_ACTIVE},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return false;
        };
        let mut exit_code = 0;
        let active = unsafe { GetExitCodeProcess(process, &mut exit_code) }.is_ok()
            && exit_code == STILL_ACTIVE.0 as u32;
        let _ = unsafe { CloseHandle(process) };
        return active;
    }
    #[cfg(not(any(unix, windows)))]
    false
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
