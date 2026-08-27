use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    PrepareProgress, PrepareStage, ServiceError, ServiceResult, SystemReleaseManifest,
    registry::replace_file,
};

use super::SERVICE_REPO;

pub(super) fn system_target() -> String {
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
    format!("{os}-{arch}")
}

pub(super) fn system_asset_name() -> String {
    let extension = if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("sabine-system-{}.{}", system_target(), extension)
}

pub(super) fn sabine_host_relative_path() -> PathBuf {
    if cfg!(target_os = "windows") {
        PathBuf::from("sabine-host.exe")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("sabine-host.app/Contents/MacOS/sabine-host")
    } else {
        PathBuf::from("sabine-host")
    }
}

pub(super) fn copy_directory(source: &Path, destination: &Path) -> ServiceResult<()> {
    if !source.is_dir() {
        return Err(ServiceError::Update(format!(
            "offline Sabine system bundle is missing {}",
            source.display()
        )));
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

pub(super) fn service_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sabine-service.exe"
    } else {
        "sabine-service"
    }
}

pub(super) fn service_daemon_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sabine-service-daemon.exe"
    } else {
        "sabine-service-daemon"
    }
}

pub(super) fn which(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(())
}

pub(super) fn download_file(
    url: &str,
    destination: &Path,
    on_progress: &mut impl FnMut(PrepareProgress),
) -> ServiceResult<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension("download");
    let response = ureq::get(url)
        .call()
        .map_err(|error| ServiceError::Update(format!("release download failed: {error}")))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let (_, body) = response.into_parts();
    let mut reader = body.into_reader();
    let mut output = fs::File::create(&temporary)?;
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        downloaded += read as u64;
        if let Some(total) = total.filter(|total| *total > 0) {
            let percent = (downloaded as f64 / total as f64 * 100.0).min(100.0);
            on_progress(PrepareProgress {
                stage: PrepareStage::Service,
                message: format!("Downloading Sabine service ({percent:.0}%)"),
                fraction: Some(0.02 + (percent as f32 / 100.0) * 0.06),
            });
        }
    }
    output.flush()?;
    output.sync_all()?;
    replace_file(&temporary, destination)?;
    Ok(())
}

pub(super) fn fetch_system_manifest() -> ServiceResult<SystemReleaseManifest> {
    let url = std::env::var("SABINE_RELEASE_MANIFEST_URL").unwrap_or_else(|_| {
        format!("https://github.com/{SERVICE_REPO}/releases/latest/download/sabine-release.json")
    });
    if !url.starts_with("https://") {
        return Err(ServiceError::Update(
            "Sabine release manifest must use HTTPS".to_string(),
        ));
    }
    let mut response = ureq::get(&url).call().map_err(|error| {
        ServiceError::Update(format!("release manifest request failed: {error}"))
    })?;
    let body = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| ServiceError::Update(format!("release manifest read failed: {error}")))?;
    serde_json::from_slice(&body)
        .map_err(|error| ServiceError::Update(format!("invalid Sabine release manifest: {error}")))
}

pub(super) fn verify_sha256(path: &Path, expected: &str) -> ServiceResult<()> {
    let mut input = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(ServiceError::Update(format!(
            "Sabine system bundle hash mismatch: expected {expected}, got {actual}"
        )))
    }
}

pub(super) fn extract_system_archive(archive: &Path, destination: &Path) -> ServiceResult<()> {
    let listing = Command::new("tar")
        .arg("-tf")
        .arg(archive)
        .output()
        .map_err(|error| {
            ServiceError::Update(format!("failed to inspect system bundle: {error}"))
        })?;
    if !listing.status.success() {
        return Err(ServiceError::Update(
            "could not inspect Sabine system bundle".to_string(),
        ));
    }
    for entry in String::from_utf8_lossy(&listing.stdout).lines() {
        let path = Path::new(entry);
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(ServiceError::Update(
                "Sabine system bundle contains an unsafe path".to_string(),
            ));
        }
    }
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| {
            ServiceError::Update(format!("failed to extract system bundle: {error}"))
        })?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| ServiceError::Update("could not extract Sabine system bundle".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_daemon_path;

    #[test]
    fn asset_names_are_platform_shaped() {
        let name = system_asset_name();
        assert!(name.starts_with("sabine-system-"));
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
