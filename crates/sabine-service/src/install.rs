use crate::{PrepareProgress, PrepareStage, ServiceError, ServiceResult, service_data_dir};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const SERVICE_REPO: &str = "Lantharos/Sabine";

pub fn cached_service_path() -> PathBuf {
    service_data_dir().join("bin").join(service_binary_name())
}

pub fn find_service_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SABINE_SERVICE_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        let candidate = directory.join(service_binary_name());
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    if let Ok(path) = which(service_binary_name()) {
        return Some(path);
    }

    let cached = cached_service_path();
    cached.is_file().then_some(cached)
}

pub fn ensure_service_executable(
    mut on_progress: impl FnMut(PrepareProgress),
) -> ServiceResult<PathBuf> {
    if let Some(path) = find_service_executable() {
        return Ok(path);
    }

    on_progress(PrepareProgress {
        stage: PrepareStage::Service,
        message: "Downloading Sabine service".to_string(),
        fraction: Some(0.02),
    });

    let destination = cached_service_path();
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension("download");
    let url = service_download_url();
    download_file(&url, &temporary, &mut on_progress)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&temporary)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&temporary, permissions)?;
    }
    fs::rename(&temporary, &destination)?;
    on_progress(PrepareProgress {
        stage: PrepareStage::Service,
        message: "Sabine service ready".to_string(),
        fraction: Some(0.08),
    });
    Ok(destination)
}

fn service_download_url() -> String {
    if let Ok(url) = std::env::var("SABINE_SERVICE_URL")
        && !url.trim().is_empty()
    {
        return url;
    }
    format!(
        "https://github.com/{SERVICE_REPO}/releases/latest/download/{}",
        service_asset_name()
    )
}

fn service_asset_name() -> String {
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
    if cfg!(target_os = "windows") {
        format!("sabine-service-{os}-{arch}.exe")
    } else {
        format!("sabine-service-{os}-{arch}")
    }
}

fn service_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sabine-service.exe"
    } else {
        "sabine-service"
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
            "failed to download Sabine service from {url}; publish a release asset or set SABINE_SERVICE_URL / SABINE_SERVICE_PATH"
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
        let name = service_asset_name();
        assert!(name.starts_with("sabine-service-"));
        assert!(name.contains("linux") || name.contains("macos") || name.contains("windows"));
    }
}
