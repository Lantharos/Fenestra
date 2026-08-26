use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::process::process_alive;
use crate::{RuntimeError, user_runtime_path};

pub struct RuntimeLease {
    path: Option<PathBuf>,
}

impl RuntimeLease {
    pub fn acquire(runtime_dir: &Path) -> Result<Self, RuntimeError> {
        if !runtime_dir.starts_with(user_runtime_path()) {
            return Ok(Self { path: None });
        }
        let directory = runtime_dir.join(".leases");
        std::fs::create_dir_all(&directory)?;
        let pid = std::process::id();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = directory.join(format!("{pid}-{nonce}.lease"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        writeln!(file, "pid={pid}")?;
        file.sync_all()?;
        Ok(Self { path: Some(path) })
    }
}

impl Drop for RuntimeLease {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(crate) fn runtime_is_leased(runtime_dir: &Path) -> Result<bool, RuntimeError> {
    let directory = runtime_dir.join(".leases");
    if !directory.is_dir() {
        return Ok(false);
    }
    let mut leased = false;
    for entry in std::fs::read_dir(&directory)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let pid = std::fs::read_to_string(&path).ok().and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("pid=")
                    .and_then(|value| value.trim().parse::<u32>().ok())
            })
        });
        if pid.is_some_and(process_alive) {
            leased = true;
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    if !leased {
        let _ = std::fs::remove_dir(directory);
    }
    Ok(leased)
}
