use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::process::Command;
#[cfg(target_os = "linux")]
use std::process::Stdio;

use crate::source_install::SourceApp;

#[cfg(target_os = "linux")]
pub fn entry(app: &SourceApp, wrapper: &Path, desktop_icon: Option<&str>) -> String {
    let icon = desktop_icon.unwrap_or(&app.id);
    let mime_types = mime_type_line(&app.mime_types);
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={} %U\nIcon={}\n{}Terminal=false\nCategories=Utility;\nStartupNotify=true\nStartupWMClass={}\n",
        desktop_value(&app.name),
        desktop_exec(wrapper),
        desktop_value(icon),
        mime_types,
        desktop_value(&app.id)
    )
}

#[cfg(target_os = "linux")]
pub fn refresh_database(applications_dir: &Path) {
    if !command_exists("update-desktop-database") {
        return;
    }
    let _ = Command::new("update-desktop-database")
        .arg(applications_dir)
        .stdin(Stdio::null())
        .status();
}

pub fn install_autostart(
    app: &SourceApp,
    wrapper: &Path,
    _desktop_icon: Option<&str>,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let directory = crate::source_install::autostart_dir()?;
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        return std::fs::write(
            directory.join(format!("{}.desktop", app.id)),
            entry(app, wrapper, _desktop_icon),
        )
        .map_err(|error| error.to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let status = Command::new("reg")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                &app.id,
                "/t",
                "REG_SZ",
                "/d",
                &wrapper.display().to_string(),
                "/f",
            ])
            .status()
            .map_err(|error| error.to_string())?;
        return status
            .success()
            .then_some(())
            .ok_or_else(|| "failed to register Windows autostart entry".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
        let directory = Path::new(&home).join("Library/LaunchAgents");
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let label = xml(&app.id);
        let executable = xml(&wrapper.display().to_string());
        return std::fs::write(
            directory.join(format!("{}.plist", app.id)),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>{executable}</string></array><key>RunAtLoad</key><true/></dict></plist>\n"
            ),
        )
        .map_err(|error| error.to_string());
    }
    #[allow(unreachable_code)]
    Err("autostart is unsupported on this platform".to_string())
}

#[cfg(target_os = "windows")]
pub fn install_windows_shortcut(
    app: &SourceApp,
    wrapper: &Path,
    _desktop_icon: Option<&str>,
) -> Result<(), String> {
    let name = powershell_string(&app.id);
    let target = powershell_string(&wrapper.display().to_string());
    let script = format!(
        "$dir=[Environment]::GetFolderPath('Programs'); $shell=New-Object -ComObject WScript.Shell; $shortcut=$shell.CreateShortcut((Join-Path $dir '{name}.lnk')); $shortcut.TargetPath='{target}'; $shortcut.Save()"
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|error| error.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "failed to create Windows Start Menu shortcut".to_string())
}

#[cfg(target_os = "macos")]
pub fn install_macos_app(
    app: &SourceApp,
    wrapper: &Path,
    _desktop_icon: Option<&str>,
) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let root = Path::new(&home)
        .join("Applications")
        .join(format!("{}.app", app.id));
    let contents = root.join("Contents");
    let macos = contents.join("MacOS");
    std::fs::create_dir_all(&macos).map_err(|error| error.to_string())?;
    std::fs::write(
        contents.join("Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{}</string><key>CFBundleName</key><string>{}</string><key>CFBundleExecutable</key><string>launch</string><key>CFBundleVersion</key><string>{}</string></dict></plist>\n",
            xml(&app.id),
            xml(&app.name),
            xml(&app.version)
        ),
    )
    .map_err(|error| error.to_string())?;
    let launch = macos.join("launch");
    std::fs::write(
        &launch,
        format!("#!/bin/sh\nexec '{}' \"$@\"\n", wrapper.display()),
    )
    .map_err(|error| error.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&launch)
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(launch, permissions).map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn mime_type_line(mime_types: &[String]) -> String {
    if mime_types.is_empty() {
        return String::new();
    }
    let values = mime_types
        .iter()
        .map(|mime_type| mime_type.trim())
        .filter(|mime_type| !mime_type.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        String::new()
    } else {
        format!("MimeType={};\n", values.join(";"))
    }
}

#[cfg(target_os = "linux")]
fn desktop_value(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

#[cfg(target_os = "linux")]
fn desktop_exec(path: &Path) -> String {
    path.display().to_string().replace(' ', "\\ ")
}

#[cfg(target_os = "linux")]
fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| {
            let candidate = path.join(name);
            candidate.is_file()
        })
    })
}

#[cfg(target_os = "windows")]
fn powershell_string(value: &str) -> String {
    value.replace('\'', "''").replace(['\r', '\n'], " ")
}

#[cfg(target_os = "macos")]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
