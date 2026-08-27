use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use crate::{
    SabineService, ServiceError, ServiceResult, ensure_service_executable,
    install::service_path_for_version, service_daemon_path, service_data_dir,
};
use sabine_runtime::configure_background_command;

#[cfg(not(target_os = "linux"))]
use sabine_runtime::background_command;

use super::{PID_FILE, autostart::install_login_autostart_with, load_policy};

#[cfg(target_os = "linux")]
use super::autostart::{install_login_autostart_with_mode, run_checked, systemd_daemon_matches};

#[cfg(target_os = "macos")]
use super::autostart::unload_macos_daemon;

const DAEMON_STATE_FILE: &str = "daemon-state.json";

pub fn ensure_daemon_running() -> ServiceResult<bool> {
    let service = ensure_service_executable(|_| {})?;
    let expected_version = service_version(&service).ok_or_else(|| {
        ServiceError::Update(format!(
            "Sabine service has no version directory: {}",
            service.display()
        ))
    })?;
    let daemon = service_daemon_path(&service);
    let login_autostart = load_policy().login_autostart;
    let matching_daemon = daemon_state()
        .is_some_and(|state| state.version == expected_version && process_alive(state.pid as i32));
    if matching_daemon {
        #[cfg(target_os = "linux")]
        if login_autostart && !systemd_daemon_matches(&daemon) {
            stop_stale_daemon()?;
            let _ = install_login_autostart_with(&service);
            if wait_for_daemon_version(&expected_version, Duration::from_secs(2)) {
                return Ok(true);
            }
        } else {
            return Ok(true);
        }
        #[cfg(not(target_os = "linux"))]
        return Ok(true);
    }
    if login_autostart {
        let installed = install_login_autostart_with(&service).is_ok();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if installed && wait_for_daemon_version(&expected_version, Duration::from_secs(2)) {
            return Ok(true);
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = installed;
    }
    stop_stale_daemon()?;
    start_daemon_at(&daemon)?;
    if wait_for_daemon_version(&expected_version, Duration::from_secs(2)) {
        Ok(true)
    } else {
        Err(ServiceError::Update(
            "Sabine service did not become ready".to_string(),
        ))
    }
}

pub fn is_daemon_running() -> bool {
    daemon_state().is_some_and(|state| process_alive(state.pid as i32))
}

pub fn start_daemon() -> ServiceResult<()> {
    ensure_daemon_running().map(|_| ())
}

pub fn run_daemon() -> ServiceResult<()> {
    let Some(_pid) = claim_daemon_pid()? else {
        return Ok(());
    };
    let service = SabineService::default();
    loop {
        match crate::stage_system_update() {
            Ok(Some(update)) => {
                begin_system_handoff(&update)?;
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => eprintln!("Sabine self-update failed: {error}"),
        }
        if let Err(error) = service.maintain() {
            eprintln!("Sabine maintenance failed: {error}");
        }
        std::thread::sleep(crate::default_maintenance_interval());
    }
}

pub fn complete_system_update(from_pid: u32, version: &str) -> ServiceResult<()> {
    #[cfg(target_os = "macos")]
    unload_macos_daemon();
    wait_for_process_exit(from_pid);
    let active = crate::cached_service_path();
    start_updated_daemon(&active)?;
    if wait_for_daemon_version(version, Duration::from_secs(20)) {
        return Ok(());
    }

    let previous = crate::rollback_system_update(version)?.ok_or_else(|| {
        ServiceError::Update(format!(
            "Sabine {version} did not start and no rollback installation is available"
        ))
    })?;
    start_updated_daemon(&previous)?;
    let previous_version = previous
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !wait_for_daemon_version(previous_version, Duration::from_secs(20)) {
        return Err(ServiceError::Update(
            "Sabine rollback daemon did not become ready".to_string(),
        ));
    }
    Err(ServiceError::Update(format!(
        "Sabine {version} failed its startup check and was rolled back"
    )))
}

fn start_updated_daemon(service: &Path) -> ServiceResult<()> {
    if load_policy().login_autostart {
        install_login_autostart_with(service)?;
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            return Ok(());
        }
    }
    start_daemon_at(&crate::service_daemon_path(service))
}

fn begin_system_handoff(update: &crate::StagedSystemUpdate) -> ServiceResult<()> {
    #[cfg(target_os = "linux")]
    {
        install_login_autostart_with_mode(&update.service, false)?;
        run_checked(Command::new("systemctl").args([
            "--user",
            "restart",
            "--no-block",
            "sabine.service",
        ]))?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        let helper = update.previous_service.as_ref().ok_or_else(|| {
            ServiceError::Update(
                "self-update has no running installation to perform handoff".into(),
            )
        })?;
        let mut command = background_command(helper);
        command
            .arg("complete-system-update")
            .arg("--from-pid")
            .arg(std::process::id().to_string())
            .arg("--version")
            .arg(&update.version)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().map_err(|error| {
            ServiceError::Update(format!("failed to start Sabine update handoff: {error}"))
        })?;
        Ok(())
    }
}

fn start_daemon_at(executable: &Path) -> ServiceResult<()> {
    let _ = fs::create_dir_all(service_data_dir());
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_background_command(&mut command);
    command.spawn().map(|_| ()).map_err(|error| {
        ServiceError::Update(format!(
            "failed to launch {}: {error}",
            executable.display()
        ))
    })
}

fn service_version(service: &Path) -> Option<String> {
    let parent = service.parent()?;
    if parent.parent()?.file_name()?.to_str()? == "versions" {
        return parent.file_name()?.to_str().map(ToString::to_string);
    }
    Some(env!("CARGO_PKG_VERSION").to_string())
}

fn stop_stale_daemon() -> ServiceResult<()> {
    let Some(state) = daemon_state().filter(|state| process_alive(state.pid as i32)) else {
        return Ok(());
    };
    let expected = service_daemon_path(&service_path_for_version(&state.version));
    let Some(actual) = process_executable(state.pid) else {
        return Err(ServiceError::Update(format!(
            "could not verify stale Sabine service {} before stopping it",
            state.pid
        )));
    };
    if !same_executable(&actual, &expected) {
        let _ = fs::remove_file(service_data_dir().join(PID_FILE));
        let _ = fs::remove_file(service_data_dir().join(DAEMON_STATE_FILE));
        return Ok(());
    }
    terminate_process(state.pid)?;
    for _ in 0..40 {
        if !process_alive(state.pid as i32) {
            let _ = fs::remove_file(service_data_dir().join(PID_FILE));
            let _ = fs::remove_file(service_data_dir().join(DAEMON_STATE_FILE));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(ServiceError::Update(format!(
        "stale Sabine service {} did not stop",
        state.pid
    )))
}

fn same_executable(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    #[cfg(windows)]
    {
        return left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy());
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(target_os = "linux")]
fn process_executable(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(target_os = "macos")]
fn process_executable(pid: u32) -> Option<PathBuf> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(windows)]
fn process_executable(pid: u32) -> Option<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows::{
        Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
        core::PWSTR,
    };
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result
        .ok()
        .map(|_| PathBuf::from(OsString::from_wide(&buffer[..length as usize])))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn process_executable(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> ServiceResult<()> {
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[cfg(windows)]
fn terminate_process(pid: u32) -> ServiceResult<()> {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess},
    };
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }
        .map_err(|error| ServiceError::Update(error.to_string()))?;
    let result = unsafe { TerminateProcess(process, 0) };
    let _ = unsafe { CloseHandle(process) };
    result.map_err(|error| ServiceError::Update(error.to_string()))
}

#[cfg(not(any(unix, windows)))]
fn terminate_process(_pid: u32) -> ServiceResult<()> {
    Err(ServiceError::Update(
        "stopping a stale Sabine service is unsupported on this platform".to_string(),
    ))
}

fn wait_for_daemon_version(version: &str, timeout: Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        if daemon_state()
            .is_some_and(|state| state.version == version && process_alive(state.pid as i32))
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn wait_for_process_exit(pid: u32) {
    while process_alive(pid as i32) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct DaemonPid {
    path: PathBuf,
    state_path: PathBuf,
    pid: u32,
}

impl Drop for DaemonPid {
    fn drop(&mut self) {
        let owns_file = fs::read_to_string(&self.path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            == Some(self.pid);
        if owns_file {
            let _ = fs::remove_file(&self.path);
        }
        let owns_state = daemon_state().is_some_and(|state| state.pid == self.pid);
        if owns_state {
            let _ = fs::remove_file(&self.state_path);
        }
    }
}

fn claim_daemon_pid() -> ServiceResult<Option<DaemonPid>> {
    fs::create_dir_all(service_data_dir())?;
    let path = service_data_dir().join(PID_FILE);
    let pid = std::process::id();
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                write!(file, "{pid}")?;
                file.sync_all()?;
                let state_path = service_data_dir().join(DAEMON_STATE_FILE);
                let state = DaemonState {
                    pid,
                    version: env!("CARGO_PKG_VERSION").to_string(),
                };
                fs::write(
                    &state_path,
                    serde_json::to_vec(&state).expect("daemon state is serializable"),
                )?;
                return Ok(Some(DaemonPid {
                    path,
                    state_path,
                    pid,
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = fs::read_to_string(&path)
                    .ok()
                    .and_then(|value| value.trim().parse::<i32>().ok());
                if existing.is_some_and(process_alive) {
                    return Ok(None);
                }
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct DaemonState {
    pid: u32,
    version: String,
}

fn daemon_state() -> Option<DaemonState> {
    serde_json::from_slice(&fs::read(service_data_dir().join(DAEMON_STATE_FILE)).ok()?).ok()
}

pub fn resolve_service_executable() -> ServiceResult<PathBuf> {
    crate::find_service_executable().ok_or_else(|| {
        ServiceError::Update(
            "sabine-service executable not found; it will be downloaded on first launch, or set SABINE_SERVICE_PATH / SABINE_RELEASE_MANIFEST_URL".to_string(),
        )
    })
}

fn process_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        if pid <= 0 {
            return false;
        }
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::{CloseHandle, STILL_ACTIVE},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        let Ok(process) =
            (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32) })
        else {
            return false;
        };
        let mut exit_code = 0;
        let active = unsafe { GetExitCodeProcess(process, &mut exit_code) }.is_ok()
            && exit_code == STILL_ACTIVE.0 as u32;
        let _ = unsafe { CloseHandle(process) };
        active
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::service_version;
    use std::path::Path;

    #[test]
    fn service_version_distinguishes_managed_and_adjacent_binaries() {
        assert_eq!(
            service_version(Path::new("Sabine/bin/versions/0.1.14/sabine-service")),
            Some("0.1.14".to_string())
        );
        assert_eq!(
            service_version(Path::new("target/debug/sabine-service")),
            Some(env!("CARGO_PKG_VERSION").to_string())
        );
    }
}
