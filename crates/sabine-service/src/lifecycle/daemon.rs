use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use crate::{
    SabineService, ServiceError, ServiceResult, ensure_service_executable, service_daemon_path,
    service_data_dir,
};

use super::{PID_FILE, autostart::install_login_autostart_with, load_policy};

#[cfg(target_os = "linux")]
use super::autostart::{install_login_autostart_with_mode, run_checked};

const DAEMON_STATE_FILE: &str = "daemon-state.json";

pub fn ensure_daemon_running() -> ServiceResult<bool> {
    if is_daemon_running() {
        return Ok(true);
    }
    start_daemon()?;
    Ok(is_daemon_running())
}

pub fn is_daemon_running() -> bool {
    #[cfg(target_os = "linux")]
    {
        if Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", "sabine.service"])
            .status()
            .is_ok_and(|status| status.success())
        {
            return true;
        }
    }

    let pid_path = service_data_dir().join(PID_FILE);
    let Ok(text) = fs::read_to_string(&pid_path) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<i32>() else {
        let _ = fs::remove_file(pid_path);
        return false;
    };
    if process_alive(pid) {
        true
    } else {
        let _ = fs::remove_file(pid_path);
        false
    }
}

pub fn start_daemon() -> ServiceResult<()> {
    let service = ensure_service_executable(|_| {})?;
    let executable = service_daemon_path(&service);
    let _ = fs::create_dir_all(service_data_dir());
    let mut command = Command::new(&executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().map_err(|error| {
        ServiceError::Update(format!(
            "failed to start Sabine service ({}): {error}",
            executable.display()
        ))
    })?;
    for _ in 0..40 {
        if is_daemon_running() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Err(ServiceError::Update(
        "Sabine service did not become ready".to_string(),
    ))
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
        let mut command = Command::new(helper);
        command
            .arg("complete-system-update")
            .arg("--from-pid")
            .arg(std::process::id().to_string())
            .arg("--version")
            .arg(&update.version)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command.spawn().map_err(|error| {
            ServiceError::Update(format!("failed to start Sabine update handoff: {error}"))
        })?;
        Ok(())
    }
}

fn start_daemon_at(executable: &Path) -> ServiceResult<()> {
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().map(|_| ()).map_err(|error| {
        ServiceError::Update(format!(
            "failed to launch {}: {error}",
            executable.display()
        ))
    })
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
