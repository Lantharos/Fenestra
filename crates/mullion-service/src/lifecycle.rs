use crate::{AppManifest, MullionService, ServiceError, ServiceResult, service_data_dir};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const POLICY_FILE: &str = "service-policy.json";
const PID_FILE: &str = "service.pid";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServicePolicy {
    /// When true, Mullion installs a login/startup entry for the background service.
    /// When false, the service is started on demand by the first Mullion app.
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

/// Make sure the shared Mullion service is available for this machine.
///
/// - Respects login-autostart policy (default on).
/// - Starts the daemon if it is not already running.
/// - Refreshes the shared runtime to the newest compatible build.
/// - Optionally registers the calling app in the service catalog.
pub fn ensure_ready(register: Option<AppManifest>) -> ServiceResult<ServiceReadyReport> {
    let policy = load_policy();
    let _ = save_policy(&policy);

    if policy.login_autostart {
        let _ = install_login_autostart();
    }

    let daemon_running = ensure_daemon_running().unwrap_or(false);
    let service = MullionService::default();
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
            .args(["--user", "is-active", "--quiet", "mullion.service"])
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
    let executable = resolve_service_executable()?;
    let _ = fs::create_dir_all(service_data_dir());
    let child = Command::new(&executable)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            ServiceError::Update(format!(
                "failed to start Mullion service ({}): {error}",
                executable.display()
            ))
        })?;
    fs::write(service_data_dir().join(PID_FILE), child.id().to_string())?;
    Ok(())
}

pub fn resolve_service_executable() -> ServiceResult<PathBuf> {
    if let Some(path) = std::env::var_os("MULLION_SERVICE_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        let candidate = directory.join(service_binary_name());
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if let Ok(path) = which(service_binary_name()) {
        return Ok(path);
    }

    Err(ServiceError::Update(
        "mullion-service executable not found; install it with `cargo install --git https://github.com/Lantharos/Mullion --package mullion-service` or set MULLION_SERVICE_PATH".to_string(),
    ))
}

fn service_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "mullion-service.exe"
    } else {
        "mullion-service"
    }
}

fn which(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(())
}

fn process_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .ok()
            .is_some_and(|output| {
                let text = String::from_utf8_lossy(&output.stdout);
                text.contains(&pid.to_string())
            })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

pub fn install_login_autostart() -> ServiceResult<()> {
    let executable = resolve_service_executable()?;
    install_login_autostart_with(&executable)
}

pub fn install_login_autostart_with(executable: &Path) -> ServiceResult<()> {
    #[cfg(target_os = "windows")]
    {
        let command = format!("\"{}\" run", executable.display());
        run_checked(Command::new("reg").args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Mullion Service",
            "/t",
            "REG_SZ",
            "/d",
            &command,
            "/f",
        ]))?;
        let uninstall = format!("\"{}\" uninstall", executable.display());
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mullion";
        for (name, value) in [
            ("DisplayName", "Mullion".to_string()),
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
            directory.join("mullion.service"),
            format!(
                "[Unit]\nDescription=Mullion runtime and app service\n\n[Service]\nExecStart={} run\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
                executable.display()
            ),
        )?;
        run_checked(Command::new("systemctl").args([
            "--user",
            "enable",
            "--now",
            "mullion.service",
        ]))?;
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| ServiceError::Update("HOME is not set".to_string()))?;
        let directory = Path::new(&home).join("Library/LaunchAgents");
        fs::create_dir_all(&directory)?;
        let path = directory.join("net.lantharos.mullion.plist");
        fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>net.lantharos.mullion</string><key>ProgramArguments</key><array><string>{}</string><string>run</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>\n",
                executable.display()
            ),
        )?;
        run_checked(Command::new("launchctl").args(["load", "-w", &path.display().to_string()]))?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        let _ = executable;
        return Err(ServiceError::Update(
            "login autostart is unsupported on this platform".to_string(),
        ));
    }
    Ok(())
}

pub fn uninstall_login_autostart() -> ServiceResult<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Mullion Service",
                "/f",
            ])
            .status();
        let _ = Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mullion",
                "/f",
            ])
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", "mullion.service"])
            .status();
        if let Some(home) = std::env::var_os("HOME") {
            let path = Path::new(&home).join(".config/systemd/user/mullion.service");
            let _ = fs::remove_file(path);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        let path = Path::new(&home).join("Library/LaunchAgents/net.lantharos.mullion.plist");
        let _ = Command::new("launchctl")
            .args(["bootout", &path.display().to_string()])
            .status();
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_file(service_data_dir().join(PID_FILE));
    Ok(())
}

fn run_checked(command: &mut Command) -> ServiceResult<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(ServiceError::Update(format!(
            "command failed with {status}"
        )))
    }
}
