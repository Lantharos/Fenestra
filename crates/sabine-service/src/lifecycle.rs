use sabine_runtime::RuntimeConfig;

use crate::{
    AppManifest, SabineService, ServiceError, ServiceResult, ensure_service_executable,
    service_daemon_path, service_data_dir,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const POLICY_FILE: &str = "service-policy.json";
const PID_FILE: &str = "service.pid";
const DAEMON_STATE_FILE: &str = "daemon-state.json";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServicePolicy {
    pub login_autostart: bool,
}

impl Default for ServicePolicy {
    fn default() -> Self {
        Self {
            login_autostart: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceReadyReport {
    pub login_autostart: bool,
    pub daemon_running: bool,
    pub runtime_version: String,
    pub registered_app: Option<String>,
}

pub fn policy_path() -> PathBuf {
    service_data_dir().join(POLICY_FILE)
}

pub fn load_policy() -> ServicePolicy {
    let path = policy_path();
    let Ok(bytes) = fs::read(&path) else {
        return ServicePolicy::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save_policy(policy: &ServicePolicy) -> ServiceResult<()> {
    fs::create_dir_all(service_data_dir())?;
    let bytes = serde_json::to_vec_pretty(policy)
        .map_err(|error| ServiceError::Update(error.to_string()))?;
    fs::write(policy_path(), bytes)?;
    Ok(())
}

pub fn set_login_autostart(enabled: bool) -> ServiceResult<ServicePolicy> {
    let mut policy = load_policy();
    policy.login_autostart = enabled;
    save_policy(&policy)?;
    if enabled {
        install_login_autostart()?;
    } else {
        uninstall_login_autostart()?;
    }
    Ok(policy)
}

pub fn ensure_ready(register: Option<AppManifest>) -> ServiceResult<ServiceReadyReport> {
    ensure_ready_with_runtime(RuntimeConfig::default(), register)
}

pub fn ensure_ready_with_runtime(
    runtime: sabine_runtime::RuntimeConfig,
    register: Option<AppManifest>,
) -> ServiceResult<ServiceReadyReport> {
    let policy = load_policy();
    let _ = save_policy(&policy);

    if policy.login_autostart {
        let _ = install_login_autostart();
    }

    let daemon_running = ensure_daemon_running().unwrap_or(false);
    let service = SabineService::default().with_runtime(runtime);
    let report = service.maintain()?;

    let registered_app = if let Some(manifest) = register {
        Some(service.register(manifest)?.manifest.id)
    } else {
        None
    };

    Ok(ServiceReadyReport {
        login_autostart: policy.login_autostart,
        daemon_running,
        runtime_version: report.runtime.version,
        registered_app,
    })
}

pub fn adopt(register: Option<AppManifest>) -> ServiceResult<ServiceReadyReport> {
    adopt_with_runtime(sabine_runtime::RuntimeConfig::default(), register)
}

pub fn adopt_with_runtime(
    runtime: sabine_runtime::RuntimeConfig,
    register: Option<AppManifest>,
) -> ServiceResult<ServiceReadyReport> {
    let policy = load_policy();
    let daemon_running = ensure_daemon_running().unwrap_or(false);
    let service = SabineService::default().with_runtime(runtime);
    let runtime = service.runtime()?;
    let registered_app = if let Some(manifest) = register {
        Some(service.register(manifest)?.manifest.id)
    } else {
        None
    };

    Ok(ServiceReadyReport {
        login_autostart: policy.login_autostart,
        daemon_running,
        runtime_version: runtime.version,
        registered_app,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareStage {
    Service,
    Runtime,
    Register,
}

#[derive(Clone, Debug)]
pub struct PrepareProgress {
    pub stage: PrepareStage,
    pub message: String,
    pub fraction: Option<f32>,
}

pub fn prepare_machine_with_progress(
    runtime: sabine_runtime::RuntimeConfig,
    register: Option<AppManifest>,
    mut on_progress: impl FnMut(PrepareProgress),
) -> ServiceResult<ServiceReadyReport> {
    on_progress(PrepareProgress {
        stage: PrepareStage::Service,
        message: "Starting Sabine service".to_string(),
        fraction: Some(0.05),
    });

    let _ = crate::ensure_service_executable(&mut on_progress)?;

    let policy = load_policy();
    let _ = save_policy(&policy);

    on_progress(PrepareProgress {
        stage: PrepareStage::Runtime,
        message: "Preparing runtime".to_string(),
        fraction: Some(0.1),
    });

    let service = SabineService::default().with_runtime(runtime);
    let runtime_info = service.ensure_runtime_with_progress(|progress| {
        let fraction = progress
            .fraction
            .map(|value| 0.1 + value.clamp(0.0, 1.0) * 0.8)
            .or(Some(0.1));
        on_progress(PrepareProgress {
            stage: PrepareStage::Runtime,
            message: progress.message,
            fraction,
        });
    })?;

    if policy.login_autostart {
        let _ = install_login_autostart();
    }
    let daemon_running = ensure_daemon_running().unwrap_or(false);

    let registered_app = if let Some(manifest) = register {
        on_progress(PrepareProgress {
            stage: PrepareStage::Register,
            message: format!("Registering {}", manifest.name),
            fraction: Some(0.95),
        });
        Some(service.register(manifest)?.manifest.id)
    } else {
        None
    };

    on_progress(PrepareProgress {
        stage: PrepareStage::Register,
        message: "Sabine is ready".to_string(),
        fraction: Some(1.0),
    });

    Ok(ServiceReadyReport {
        login_autostart: policy.login_autostart,
        daemon_running,
        runtime_version: runtime_info.version,
        registered_app,
    })
}

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

pub fn install_login_autostart() -> ServiceResult<()> {
    let executable = ensure_service_executable(|_| {})?;
    install_login_autostart_with(&executable)
}

pub fn install_login_autostart_with(executable: &Path) -> ServiceResult<()> {
    install_login_autostart_with_mode(executable, true)
}

fn install_login_autostart_with_mode(
    executable: &Path,
    restart_mismatched_daemon: bool,
) -> ServiceResult<()> {
    #[cfg(not(target_os = "linux"))]
    let _ = restart_mismatched_daemon;
    let daemon = service_daemon_path(executable);
    if !daemon.is_file() {
        return Err(ServiceError::Update(format!(
            "Sabine service daemon not found at {}",
            daemon.display()
        )));
    }
    #[cfg(target_os = "windows")]
    {
        let daemon_literal = daemon.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$identity=[Security.Principal.WindowsIdentity]::GetCurrent();\
             $action=New-ScheduledTaskAction -Execute '{daemon_literal}';\
             $trigger=New-ScheduledTaskTrigger -AtLogOn -User $identity.Name;\
             $principal=New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType Interactive -RunLevel Limited;\
             $settings=New-ScheduledTaskSettingsSet -Hidden -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew;\
             Register-ScheduledTask -TaskName 'Sabine Service' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null"
        );
        run_checked(Command::new("powershell.exe").args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ]))?;
        let _ = Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Sabine Service",
                "/f",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let uninstall = format!("\"{}\" uninstall", executable.display());
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Sabine";
        for (name, value) in [
            ("DisplayName", "Sabine".to_string()),
            ("DisplayVersion", env!("CARGO_PKG_VERSION").to_string()),
            ("Publisher", "Lantharos".to_string()),
            ("UninstallString", uninstall),
        ] {
            run_checked(
                Command::new("reg")
                    .args(["add", key, "/v", name, "/t", "REG_SZ", "/d", &value, "/f"]),
            )?;
        }
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| ServiceError::Update("HOME is not set".to_string()))?;
        let directory = Path::new(&home).join(".config/systemd/user");
        fs::create_dir_all(&directory)?;
        fs::write(
            directory.join("sabine.service"),
            format!(
                "[Unit]\nDescription=Sabine runtime and app service\n\n[Service]\nExecStart={}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
                daemon.display()
            ),
        )?;
        run_checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        run_checked(Command::new("systemctl").args([
            "--user",
            "enable",
            "--now",
            "sabine.service",
        ]))?;
        if restart_mismatched_daemon && !systemd_daemon_matches(&daemon) {
            run_checked(Command::new("systemctl").args(["--user", "restart", "sabine.service"]))?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| ServiceError::Update("HOME is not set".to_string()))?;
        let directory = Path::new(&home).join("Library/LaunchAgents");
        fs::create_dir_all(&directory)?;
        let path = directory.join("net.lantharos.sabine.plist");
        fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>net.lantharos.sabine</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>\n",
                daemon.display()
            ),
        )?;
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let service = format!("{domain}/net.lantharos.sabine");
        let _ = Command::new("launchctl")
            .args(["bootout", &service])
            .status();
        run_checked(Command::new("launchctl").args([
            "bootstrap",
            &domain,
            &path.display().to_string(),
        ]))?;
        run_checked(Command::new("launchctl").args(["enable", &service]))?;
        run_checked(Command::new("launchctl").args(["kickstart", "-k", &service]))?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        let _ = daemon;
        return Err(ServiceError::Update(
            "login autostart is unsupported on this platform".to_string(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn systemd_daemon_matches(expected: &Path) -> bool {
    let Ok(output) = Command::new("systemctl")
        .args([
            "--user",
            "show",
            "--property=MainPID",
            "--value",
            "sabine.service",
        ])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Ok(pid) = std::str::from_utf8(&output.stdout)
        .map(str::trim)
        .unwrap_or_default()
        .parse::<u32>()
    else {
        return false;
    };
    fs::canonicalize(format!("/proc/{pid}/exe")).ok() == fs::canonicalize(expected).ok()
}

#[cfg(target_os = "macos")]
fn unload_macos_daemon() {
    let service = format!("gui/{}/net.lantharos.sabine", unsafe { libc::getuid() });
    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .status();
}

pub fn uninstall_login_autostart() -> ServiceResult<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("schtasks")
            .args(["/Delete", "/TN", "Sabine Service", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Sabine Service",
                "/f",
            ])
            .status();
        let _ = Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Sabine",
                "/f",
            ])
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", "sabine.service"])
            .status();
        if let Some(home) = std::env::var_os("HOME") {
            let path = Path::new(&home).join(".config/systemd/user/sabine.service");
            let _ = fs::remove_file(path);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        let path = Path::new(&home).join("Library/LaunchAgents/net.lantharos.sabine.plist");
        let _ = Command::new("launchctl")
            .args(["bootout", &path.display().to_string()])
            .status();
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_file(service_data_dir().join(PID_FILE));
    Ok(())
}

fn run_checked(command: &mut Command) -> ServiceResult<()> {
    // Windows `reg.exe` prints "The operation completed successfully." to stdout
    // on every successful write; keep that noise out of the setup UI.
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(ServiceError::Update(format!(
            "command failed with {status}"
        )))
    }
}
