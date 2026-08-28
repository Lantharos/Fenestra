use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    ServiceError, ServiceResult, ensure_service_executable, service_daemon_path, service_data_dir,
};
use sabine_runtime::configure_background_command;

#[cfg(target_os = "windows")]
use sabine_runtime::background_command;

use super::PID_FILE;

pub fn install_login_autostart() -> ServiceResult<()> {
    let executable = ensure_service_executable(|_| {})?;
    install_login_autostart_with(&executable)
}

pub fn install_login_autostart_with(executable: &Path) -> ServiceResult<()> {
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
        run_checked(background_command("powershell.exe").args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ]))?;
        let _ = background_command("reg")
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
            ("DisplayVersion", crate::SABINE_VERSION.to_string()),
            ("Publisher", "Lantharos".to_string()),
            ("UninstallString", uninstall),
        ] {
            run_checked(
                background_command("reg")
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
        let _ = Command::new("systemctl")
            .args(["--user", "reset-failed", "sabine.service"])
            .status();
        run_checked(Command::new("systemctl").args([
            "--user",
            "enable",
            "--now",
            "sabine.service",
        ]))?;
        if !systemd_daemon_matches(&daemon) {
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
pub(super) fn systemd_daemon_matches(expected: &Path) -> bool {
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
pub(super) fn unload_macos_daemon() {
    let service = format!("gui/{}/net.lantharos.sabine", unsafe { libc::getuid() });
    let _ = Command::new("launchctl")
        .args(["bootout", &service])
        .status();
}

pub fn uninstall_login_autostart() -> ServiceResult<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = background_command("schtasks")
            .args(["/Delete", "/TN", "Sabine Service", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = background_command("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Sabine Service",
                "/f",
            ])
            .status();
        let _ = background_command("reg")
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

pub(super) fn run_checked(command: &mut Command) -> ServiceResult<()> {
    // Windows `reg.exe` prints "The operation completed successfully." to stdout
    // on every successful write; keep that noise out of the setup UI.
    configure_background_command(command);
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
