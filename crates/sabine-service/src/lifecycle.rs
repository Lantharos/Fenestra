use crate::{
    AppManifest, SabineService, ServiceError, ServiceResult, ensure_service_executable,
    service_data_dir,
};
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
    let policy = load_policy();
    let _ = save_policy(&policy);

    if policy.login_autostart {
        let _ = install_login_autostart();
    }

    let daemon_running = ensure_daemon_running().unwrap_or(false);
    let service = SabineService::default();
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
    let policy = load_policy();
    let daemon_running = ensure_daemon_running().unwrap_or(false);
    let service = SabineService::default();
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
    if policy.login_autostart {
        let _ = install_login_autostart();
    }
    let daemon_running = ensure_daemon_running().unwrap_or(false);

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
    let executable = ensure_service_executable(|_| {})?;
    let _ = fs::create_dir_all(service_data_dir());
    let child = Command::new(&executable)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            ServiceError::Update(format!(
                "failed to start Sabine service ({}): {error}",
                executable.display()
            ))
        })?;
    fs::write(service_data_dir().join(PID_FILE), child.id().to_string())?;
    Ok(())
}

pub fn resolve_service_executable() -> ServiceResult<PathBuf> {
    crate::find_service_executable().ok_or_else(|| {
        ServiceError::Update(
            "sabine-service executable not found; it will be downloaded on first launch, or set SABINE_SERVICE_PATH / SABINE_SERVICE_URL".to_string(),
        )
    })
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
    let executable = ensure_service_executable(|_| {})?;
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
            "Sabine Service",
            "/t",
            "REG_SZ",
            "/d",
            &command,
            "/f",
        ]))?;
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
                "[Unit]\nDescription=Sabine runtime and app service\n\n[Service]\nExecStart={} run\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
                executable.display()
            ),
        )?;
        run_checked(Command::new("systemctl").args([
            "--user",
            "enable",
            "--now",
            "sabine.service",
        ]))?;
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
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>net.lantharos.sabine</string><key>ProgramArguments</key><array><string>{}</string><string>run</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>\n",
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
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(ServiceError::Update(format!(
            "command failed with {status}"
        )))
    }
}
