use crate::{PrepareProgress, PrepareStage, ServiceError, ServiceResult, service_data_dir};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const SERVICE_REPO: &str = "Lantharos/Sabine";
const SERVICE_GIT_URL: &str = "https://github.com/Lantharos/Sabine";

pub fn cached_service_path() -> PathBuf {
    service_data_dir().join("bin").join(service_binary_name())
}

pub fn service_daemon_path(service: &Path) -> PathBuf {
    service.with_file_name(service_daemon_binary_name())
}

fn complete_service_at(path: PathBuf) -> Option<PathBuf> {
    (path.is_file() && service_daemon_path(&path).is_file()).then_some(path)
}

pub fn find_service_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SABINE_SERVICE_PATH") {
        let path = PathBuf::from(path);
        if let Some(path) = complete_service_at(path) {
            return Some(path);
        }
    }

    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        let candidate = directory.join(service_binary_name());
        if let Some(candidate) = complete_service_at(candidate) {
            return Some(candidate);
        }
    }

    if let Ok(path) = which(service_binary_name())
        && let Some(path) = complete_service_at(path)
    {
        return Some(path);
    }

    complete_service_at(cached_service_path())
}

pub fn ensure_service_executable(
    mut on_progress: impl FnMut(PrepareProgress),
) -> ServiceResult<PathBuf> {
    if let Some(path) = find_service_executable() {
        return Ok(path);
    }

    let destination = cached_service_path();
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    on_progress(PrepareProgress {
        stage: PrepareStage::Service,
        message: "Downloading Sabine service".to_string(),
        fraction: Some(0.02),
    });

    match try_download_release_service(&destination, &mut on_progress) {
        Ok(()) => {
            on_progress(PrepareProgress {
                stage: PrepareStage::Service,
                message: "Sabine service ready".to_string(),
                fraction: Some(0.08),
            });
            Ok(destination)
        }
        Err(download_error) => {
            on_progress(PrepareProgress {
                stage: PrepareStage::Service,
                message: "Release asset missing; building Sabine service from GitHub".to_string(),
                fraction: Some(0.03),
            });
            install_service_via_cargo(&destination, &mut on_progress).map_err(|cargo_error| {
                ServiceError::Update(format!(
                    "{download_error}; cargo install also failed: {cargo_error}"
                ))
            })?;
            on_progress(PrepareProgress {
                stage: PrepareStage::Service,
                message: "Sabine service ready".to_string(),
                fraction: Some(0.08),
            });
            Ok(destination)
        }
    }
}

fn try_download_release_service(
    destination: &Path,
    on_progress: &mut impl FnMut(PrepareProgress),
) -> ServiceResult<()> {
    let temporary = destination.with_extension("download");
    let daemon_destination = service_daemon_path(destination);
    let daemon_temporary = daemon_destination.with_extension("download");
    let url = service_download_url(false);
    download_file(&url, &temporary, on_progress)?;
    let daemon_url = service_download_url(true);
    if let Err(error) = download_file(&daemon_url, &daemon_temporary, on_progress) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    finalize_service_binary(&temporary, destination)?;
    finalize_service_binary(&daemon_temporary, &daemon_destination)?;
    Ok(())
}

fn install_service_via_cargo(
    destination: &Path,
    on_progress: &mut impl FnMut(PrepareProgress),
) -> ServiceResult<()> {
    if which("cargo").is_err() {
        return Err(ServiceError::Update(
            "Rust/cargo is required to build sabine-service until GitHub Releases publish binaries. \
Install Rust from https://rustup.rs, or set SABINE_SERVICE_PATH / SABINE_SERVICE_URL."
                .into(),
        ));
    }

    on_progress(PrepareProgress {
        stage: PrepareStage::Service,
        message: "Building Sabine service with cargo (this can take a few minutes)".to_string(),
        fraction: Some(0.04),
    });

    let cargo_root = destination
        .parent()
        .map(|parent| parent.join(".cargo-root"))
        .ok_or_else(|| ServiceError::Update("invalid service destination".into()))?;
    if cargo_root.exists() {
        let _ = fs::remove_dir_all(&cargo_root);
    }
    fs::create_dir_all(&cargo_root)?;

    let status = Command::new("cargo")
        .args(["install", "--git", SERVICE_GIT_URL, "--force", "--root"])
        .arg(&cargo_root)
        .arg("--package")
        .arg("sabine-service")
        .status()
        .map_err(|error| ServiceError::Update(format!("failed to run cargo: {error}")))?;
    if !status.success() {
        let _ = fs::remove_dir_all(&cargo_root);
        return Err(ServiceError::Update(format!(
            "cargo install --git {SERVICE_GIT_URL} --package sabine-service failed with {status}"
        )));
    }

    let installed = cargo_root.join("bin").join(service_binary_name());
    let installed_daemon = cargo_root.join("bin").join(service_daemon_binary_name());
    if !installed.is_file() || !installed_daemon.is_file() {
        let _ = fs::remove_dir_all(&cargo_root);
        return Err(ServiceError::Update(format!(
            "cargo install succeeded but {} and {} were not both created",
            installed.display(),
            installed_daemon.display()
        )));
    }
    fs::copy(&installed, destination).map_err(|error| {
        ServiceError::Update(format!(
            "failed to copy service binary to {}: {error}",
            destination.display()
        ))
    })?;
    let daemon_destination = service_daemon_path(destination);
    fs::copy(&installed_daemon, &daemon_destination).map_err(|error| {
        ServiceError::Update(format!(
            "failed to copy service daemon to {}: {error}",
            daemon_destination.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [destination, daemon_destination.as_path()] {
            let mut permissions = fs::metadata(path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions)?;
        }
    }
    let _ = fs::remove_dir_all(&cargo_root);
    Ok(())
}

fn finalize_service_binary(temporary: &Path, destination: &Path) -> ServiceResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(temporary)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(temporary, permissions)?;
    }
    fs::rename(temporary, destination)?;
    Ok(())
}

fn service_download_url(daemon: bool) -> String {
    let override_name = if daemon {
        "SABINE_SERVICE_DAEMON_URL"
    } else {
        "SABINE_SERVICE_URL"
    };
    if let Ok(url) = std::env::var(override_name)
        && !url.trim().is_empty()
    {
        return url;
    }
    format!(
        "https://github.com/{SERVICE_REPO}/releases/latest/download/{}",
        service_asset_name(daemon)
    )
}

fn service_asset_name(daemon: bool) -> String {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    let executable = if daemon {
        "sabine-service-daemon"
    } else {
        "sabine-service"
    };
    if cfg!(target_os = "windows") {
        format!("{executable}-{os}-{arch}.exe")
    } else {
        format!("{executable}-{os}-{arch}")
    }
}

fn service_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sabine-service.exe"
    } else {
        "sabine-service"
    }
}

fn service_daemon_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sabine-service-daemon.exe"
    } else {
        "sabine-service-daemon"
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

fn download_file(
    url: &str,
    destination: &Path,
    on_progress: &mut impl FnMut(PrepareProgress),
) -> ServiceResult<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut child = Command::new("curl")
        .args([
            "-L",
            "--fail",
            "-o",
            destination.to_string_lossy().as_ref(),
            url,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ServiceError::Update(format!("failed to run curl: {error}")))?;
    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        for line in reader.split(b'\r').flatten() {
            if let Some(percent) = parse_curl_percent(&line) {
                on_progress(PrepareProgress {
                    stage: PrepareStage::Service,
                    message: format!("Downloading Sabine service ({percent:.0}%)"),
                    fraction: Some(0.02 + (percent / 100.0) * 0.06),
                });
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| ServiceError::Update(format!("curl wait failed: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        let _ = fs::remove_file(destination);
        Err(ServiceError::Update(format!(
            "failed to download Sabine service from {url}"
        )))
    }
}

fn parse_curl_percent(line: &[u8]) -> Option<f32> {
    let text = String::from_utf8_lossy(line);
    let token = text.split_whitespace().next()?;
    let percent = token.parse::<f32>().ok()?;
    (0.0..=100.0).contains(&percent).then_some(percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_are_platform_shaped() {
        let name = service_asset_name(false);
        assert!(name.starts_with("sabine-service-"));
        assert!(name.contains("linux") || name.contains("macos") || name.contains("windows"));
    }

    #[test]
    fn daemon_lives_beside_the_service_cli() {
        let path = service_daemon_path(Path::new("/tmp/sabine-service"));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(service_daemon_binary_name())
        );
    }
}
