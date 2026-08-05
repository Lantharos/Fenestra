use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::detect::{detect_package, is_runtime_dir};
use crate::download::{
    download_file, extract_archive, first_extracted_runtime_dir, latest_install_plan, verify_sha1,
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
        None,
        "Preparing runtime install",
    ));
    remove_user_minimal_runtime_if_client_requested_with_progress(config, &mut progress)?;

    let plan = latest_install_plan(config)?;
    if plan.install_dir.is_dir() {
        return Ok(RuntimeInfo {
            package: config.package,
            version: plan.version,
            location: RuntimeLocation::UserLocal(plan.install_dir),
            verified: true,
        });
    }

    let work_dir = user_runtime_path().join(".installing");
    if work_dir.exists() {
        std::fs::remove_dir_all(&work_dir)?;
    }
    std::fs::create_dir_all(&work_dir)?;

    let archive_path = work_dir.join(&plan.archive_name);
    download_file(&plan.url, &archive_path, &mut progress)?;
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Verifying,
        None,
        "Verifying CEF archive",
    ));
    verify_sha1(&archive_path, &plan.sha1)?;
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Extracting,
        None,
        "Extracting CEF runtime",
    ));
    extract_archive(&archive_path, &work_dir)?;

    let extracted = first_extracted_runtime_dir(&work_dir).ok_or_else(|| {
        RuntimeError::InstallationFailed(
            "download did not contain a CEF runtime directory".to_string(),
        )
    })?;
    if plan.install_dir.exists() {
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::RemovingOldRuntime,
            None,
            "Removing previous runtime",
        ));
        std::fs::remove_dir_all(&plan.install_dir)?;
    }
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Installing,
        None,
        "Installing CEF runtime",
    ));
    std::fs::rename(&extracted, &plan.install_dir)?;
    std::fs::write(plan.install_dir.join("VERSION"), &plan.version)?;
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
        let mut announced_wait = false;

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
                    if !announced_wait {
                        progress(RuntimeInstallProgress::new(
                            RuntimeInstallStep::Preparing,
                            None,
                            "Waiting for another Mullion runtime install",
                        ));
                        announced_wait = true;
                    }
                    if started.elapsed() >= INSTALL_LOCK_TIMEOUT {
                        return Err(RuntimeError::InstallationFailed(format!(
                            "timed out waiting for runtime install lock at {}",
                            path.display()
                        )));
                    }
                    thread::sleep(Duration::from_millis(100));
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

fn lock_is_stale(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= INSTALL_LOCK_STALE_AFTER)
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
