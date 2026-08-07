use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::detect::{detect_package, is_runtime_dir};
use crate::download::{
    download_file, extract_archive, first_extracted_runtime_dir, latest_install_plan,
    verify_sha1_with_progress,
};
use crate::error::RuntimeError;
use crate::paths::user_runtime_path;
use crate::resolve::resolve_runtime;
use crate::types::{
    RuntimeConfig, RuntimeInfo, RuntimeInstallProgress, RuntimeInstallStep, RuntimeLocation,
    RuntimePackage,
};
use crate::version::{detect_version, runtime_sort_key};

const INSTALL_LOCK_TIMEOUT: Duration = Duration::from_secs(600);
const INSTALL_LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);
const INSTALL_LOCK_WAIT_HEARTBEAT: Duration = Duration::from_secs(3);

pub fn install_user_runtime(config: &RuntimeConfig) -> Result<RuntimeInfo, RuntimeError> {
    install_user_runtime_with_progress(config, |_| {})
}

pub fn install_user_runtime_with_progress(
    config: &RuntimeConfig,
    progress: impl FnMut(RuntimeInstallProgress),
) -> Result<RuntimeInfo, RuntimeError> {
    install_user_runtime_inner(config, progress, false)
}

pub fn update_user_runtime_with_progress(
    config: &RuntimeConfig,
    progress: impl FnMut(RuntimeInstallProgress),
) -> Result<RuntimeInfo, RuntimeError> {
    install_user_runtime_inner(config, progress, true)
}

fn install_user_runtime_inner(
    config: &RuntimeConfig,
    mut progress: impl FnMut(RuntimeInstallProgress),
    require_latest: bool,
) -> Result<RuntimeInfo, RuntimeError> {
    if !config.allow_user_install {
        return Err(RuntimeError::DownloadsDisabled);
    }
    std::fs::create_dir_all(user_runtime_path())?;
    let _lock = RuntimeInstallLock::acquire(&mut progress)?;
    if !require_latest && let Ok(runtime) = resolve_runtime(config) {
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::Complete,
            Some(1.0),
            "Runtime ready",
        ));
        return Ok(runtime);
    }
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Preparing,
        Some(0.02),
        "Preparing runtime",
    ));
    remove_user_minimal_runtime_if_client_requested_with_progress(config, &mut progress)?;

    let plan = latest_install_plan(config)?;
    if plan.install_dir.is_dir() {
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::Complete,
            Some(1.0),
            "Runtime ready",
        ));
        return Ok(RuntimeInfo {
            package: config.package,
            version: plan.version,
            location: RuntimeLocation::UserLocal(plan.install_dir),
            verified: true,
        });
    }

    let work_dir = user_runtime_path().join(".installing");
    std::fs::create_dir_all(&work_dir)?;
    // Drop leftover extract trees from a previous attempt, but keep a matching
    // archive so a crashed install can resume without re-downloading ~600MB.
    cleanup_install_work_dir(&work_dir, &plan.archive_name)?;

    let archive_path = work_dir.join(&plan.archive_name);
    let mut archive_ready = false;
    if archive_path.is_file() {
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::Verifying,
            Some(0.70),
            "Checking downloaded runtime archive",
        ));
        match verify_sha1_with_progress(&archive_path, &plan.sha1, &mut progress) {
            Ok(()) => archive_ready = true,
            Err(_) => {
                progress(RuntimeInstallProgress::new(
                    RuntimeInstallStep::Downloading,
                    Some(0.05),
                    "Downloaded archive incomplete; re-downloading",
                ));
                let _ = std::fs::remove_file(&archive_path);
            }
        }
    }
    if archive_ready {
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::Verifying,
            Some(0.72),
            "Resuming with downloaded runtime archive",
        ));
    } else {
        download_file(&plan.url, &archive_path, &mut progress)?;
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::Verifying,
            Some(0.72),
            "Verifying runtime",
        ));
        verify_sha1_with_progress(&archive_path, &plan.sha1, &mut progress)?;
    }
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Extracting,
        Some(0.78),
        "Extracting runtime",
    ));
    extract_archive_with_progress(&archive_path, &work_dir, &mut progress)?;

    let extracted = first_extracted_runtime_dir(&work_dir).ok_or_else(|| {
        RuntimeError::InstallationFailed("download did not contain a runtime directory".to_string())
    })?;
    if plan.install_dir.exists() {
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::RemovingOldRuntime,
            Some(0.93),
            "Removing previous runtime",
        ));
        std::fs::remove_dir_all(&plan.install_dir)?;
    }
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Installing,
        Some(0.96),
        "Installing runtime",
    ));
    std::fs::rename(&extracted, &plan.install_dir)?;
    std::fs::write(plan.install_dir.join("VERSION"), &plan.version)?;
    if config.package == RuntimePackage::Standard
        && !crate::host::standard_sdk_present(&plan.install_dir)
    {
        let path = plan.install_dir.clone();
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_dir_all(&work_dir);
        return Err(RuntimeError::InstallationFailed(format!(
            "extracted CEF archive at {} is missing the standard SDK layout \
             (cmake/, include/cef_version.h, libcef_dll/, Release/libcef.*). \
             On Windows, ensure `tar` is the OS/bsdtar build — Git's GNU tar \
             mishandles drive-letter extract paths.",
            path.display()
        )));
    }
    let _ = std::fs::remove_dir_all(&work_dir);
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Complete,
        Some(1.0),
        "Runtime ready",
    ));

    Ok(RuntimeInfo {
        package: config.package,
        version: plan.version,
        location: RuntimeLocation::UserLocal(plan.install_dir),
        verified: true,
    })
}

fn cleanup_install_work_dir(work_dir: &Path, keep_archive_name: &str) -> Result<(), RuntimeError> {
    if !work_dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(work_dir)? {
        let path = entry?.path();
        let keep = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == keep_archive_name);
        if keep {
            continue;
        }
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn extract_archive_with_progress(
    archive: &std::path::Path,
    destination: &std::path::Path,
    progress: &mut impl FnMut(RuntimeInstallProgress),
) -> Result<(), RuntimeError> {
    let archive = archive.to_path_buf();
    let destination = destination.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let result = extract_archive(&archive, &destination);
        let _ = tx.send(());
        result
    });

    let mut fraction = 0.78_f32;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(()) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                fraction = (fraction + 0.008).min(0.92);
                progress(RuntimeInstallProgress::new(
                    RuntimeInstallStep::Extracting,
                    Some(fraction),
                    "Extracting runtime",
                ));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    worker.join().unwrap_or_else(|_| {
        Err(RuntimeError::InstallationFailed(
            "runtime extraction worker failed".to_string(),
        ))
    })?;
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Extracting,
        Some(0.93),
        "Extracting runtime",
    ));
    Ok(())
}

pub fn remove_user_minimal_runtime_if_client_requested(
    config: &RuntimeConfig,
) -> Result<(), RuntimeError> {
    remove_user_minimal_runtime_if_client_requested_with_progress(config, |_| {})
}

pub fn prune_user_runtimes(
    config: &RuntimeConfig,
    keep_latest: usize,
) -> Result<usize, RuntimeError> {
    std::fs::create_dir_all(user_runtime_path())?;
    let _lock = RuntimeInstallLock::acquire(|_| {})?;
    let base = user_runtime_path();
    if !base.is_dir() {
        return Ok(0);
    }

    let mut runtimes = std::fs::read_dir(base)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && is_runtime_dir(path))
        .filter(|path| detect_package(path) == config.package)
        .collect::<Vec<_>>();
    runtimes.sort_by_key(|path| std::cmp::Reverse(runtime_sort_key(path)));

    let mut removed = 0;
    for path in runtimes.into_iter().skip(keep_latest.max(1)) {
        std::fs::remove_dir_all(path)?;
        removed += 1;
    }
    Ok(removed)
}

pub fn remove_user_runtime_version(
    config: &RuntimeConfig,
    version: &str,
) -> Result<bool, RuntimeError> {
    std::fs::create_dir_all(user_runtime_path())?;
    let _lock = RuntimeInstallLock::acquire(|_| {})?;
    let base = user_runtime_path();
    if !base.is_dir() {
        return Ok(false);
    }

    let mut removed = false;
    for path in std::fs::read_dir(base)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && is_runtime_dir(path))
        .filter(|path| detect_package(path) == config.package)
        .filter(|path| detect_version(path) == version)
    {
        std::fs::remove_dir_all(path)?;
        removed = true;
    }
    Ok(removed)
}

struct RuntimeInstallLock {
    path: PathBuf,
}

impl RuntimeInstallLock {
    fn acquire(mut progress: impl FnMut(RuntimeInstallProgress)) -> Result<Self, RuntimeError> {
        let base = user_runtime_path();
        std::fs::create_dir_all(&base)?;
        let path = base.join(".install.lock");
        let started = Instant::now();
        let mut last_heartbeat = Instant::now()
            .checked_sub(INSTALL_LOCK_WAIT_HEARTBEAT)
            .unwrap_or_else(Instant::now);

        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    let _ = writeln!(file, "started={}", unix_timestamp_secs());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        progress(RuntimeInstallProgress::new(
                            RuntimeInstallStep::Preparing,
                            None,
                            "Taking over abandoned runtime install",
                        ));
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if last_heartbeat.elapsed() >= INSTALL_LOCK_WAIT_HEARTBEAT {
                        let waited = started.elapsed().as_secs();
                        progress(RuntimeInstallProgress::new(
                            RuntimeInstallStep::Preparing,
                            None,
                            format!("Waiting for another Sabine runtime install ({waited}s)"),
                        ));
                        last_heartbeat = Instant::now();
                    }
                    if started.elapsed() >= INSTALL_LOCK_TIMEOUT {
                        return Err(RuntimeError::InstallationFailed(format!(
                            "timed out waiting for runtime install lock at {}",
                            path.display()
                        )));
                    }
                    thread::sleep(Duration::from_millis(200));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for RuntimeInstallLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn remove_user_minimal_runtime_if_client_requested_with_progress(
    config: &RuntimeConfig,
    mut progress: impl FnMut(RuntimeInstallProgress),
) -> Result<(), RuntimeError> {
    if config.package == RuntimePackage::Minimal {
        return Ok(());
    }

    let base = user_runtime_path();
    if !base.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(base)? {
        let path = entry?.path();
        if !path.is_dir() || detect_package(&path) != RuntimePackage::Minimal {
            continue;
        }
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::RemovingOldRuntime,
            None,
            "Removing minimal runtime",
        ));
        std::fs::remove_dir_all(path)?;
    }

    Ok(())
}

fn lock_is_stale(path: &Path) -> bool {
    if !lock_holder_alive(path) {
        return true;
    }
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= INSTALL_LOCK_STALE_AFTER)
}

fn lock_holder_alive(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(pid) = contents.lines().find_map(|line| {
        line.strip_prefix("pid=")
            .and_then(|value| value.trim().parse::<u32>().ok())
    }) else {
        return false;
    };
    process_alive(pid)
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // signal 0 checks existence / permission without killing
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        // Without a cheap PID probe, keep the age-based stale timeout.
        let _ = pid;
        true
    }
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
