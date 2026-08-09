use sabine_runtime::{prune_user_runtimes, update_user_runtime_with_progress};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

use crate::registry::{RegistryLock, SabineService};
use crate::types::{
    AppArtifact, AppReleaseManifest, MaintenanceReport, ServiceError, ServiceResult, UpdatePolicy,
    is_https_url, platform_target, unix_timestamp, version_is_newer,
};

impl SabineService {
    pub fn maintain(&self) -> ServiceResult<MaintenanceReport> {
        let runtime = update_user_runtime_with_progress(&self.runtime, |_| {})?;
        let pruned_runtimes = prune_user_runtimes(2)?;
        let apps = self.apps()?;
        let mut updated_apps = Vec::new();
        let mut update_failures = Vec::new();
        for app in &apps {
            let Some(update) = &app.manifest.update else {
                continue;
            };
            if update.policy != UpdatePolicy::Automatic {
                continue;
            }
            match self.update_app(&app.manifest.id) {
                Ok(true) => updated_apps.push(app.manifest.id.clone()),
                Ok(false) => {}
                Err(error) => update_failures.push(format!("{}: {error}", app.manifest.id)),
            }
        }
        Ok(MaintenanceReport {
            runtime,
            pruned_runtimes,
            registered_apps: apps.len(),
            automatic_updates: apps
                .iter()
                .filter(|app| {
                    app.manifest
                        .update
                        .as_ref()
                        .is_some_and(|update| update.policy == UpdatePolicy::Automatic)
                })
                .count(),
            updated_apps,
            update_failures,
        })
    }

    pub fn update_app(&self, id: &str) -> ServiceResult<bool> {
        let app = self.app(id)?;
        let update = app
            .manifest
            .update
            .as_ref()
            .ok_or_else(|| ServiceError::Update("app has no update source".to_string()))?;
        let release = fetch_release(&update.manifest_url)?;
        if release.channel != update.channel
            || !version_is_newer(&release.version, &app.manifest.version)
        {
            return Ok(false);
        }
        let target = platform_target();
        let artifact = release
            .artifacts
            .get(target)
            .ok_or_else(|| ServiceError::Update(format!("release has no artifact for {target}")))?;
        validate_artifact(artifact)?;
        let release_dir = self
            .root
            .join("apps")
            .join(id)
            .join("releases")
            .join(&release.version);
        let executable = release_dir.join(&artifact.executable);
        if !executable.is_file() {
            install_artifact(&self.root, id, &release.version, artifact, &release_dir)?;
        }
        if !executable.is_file() {
            return Err(ServiceError::Update(format!(
                "artifact did not contain {}",
                artifact.executable.display()
            )));
        }
        let _lock = RegistryLock::acquire(&self.root)?;
        let mut registry = self.load_registry()?;
        let registered = registry
            .apps
            .get_mut(id)
            .ok_or_else(|| ServiceError::AppNotFound(id.to_string()))?;
        if !version_is_newer(&release.version, &registered.manifest.version) {
            return Ok(false);
        }
        registered.manifest.version = release.version;
        registered.manifest.executable = executable;
        registered.updated_at = unix_timestamp();
        self.save_registry(&registry)?;
        Ok(true)
    }
}

fn fetch_release(url: &str) -> ServiceResult<AppReleaseManifest> {
    if !is_https_url(url) {
        return Err(ServiceError::Update(
            "release manifest URL must use HTTPS".to_string(),
        ));
    }
    let output = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error", url])
        .output()
        .map_err(|error| ServiceError::Update(format!("failed to run curl: {error}")))?;
    if !output.status.success() {
        return Err(ServiceError::Update(format!(
            "release request failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| ServiceError::Update(format!("invalid release manifest: {error}")))
}

fn validate_artifact(artifact: &AppArtifact) -> ServiceResult<()> {
    if !is_https_url(&artifact.url) {
        return Err(ServiceError::Update(
            "artifact URL must use HTTPS".to_string(),
        ));
    }
    if artifact.sha256.len() != 64 || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ServiceError::Update(
            "artifact SHA-256 is invalid".to_string(),
        ));
    }
    if !safe_relative_path(&artifact.executable) {
        return Err(ServiceError::Update(
            "artifact executable path is unsafe".to_string(),
        ));
    }
    Ok(())
}

fn install_artifact(
    root: &Path,
    id: &str,
    version: &str,
    artifact: &AppArtifact,
    release_dir: &Path,
) -> ServiceResult<()> {
    let downloads = root.join("downloads").join(id);
    std::fs::create_dir_all(&downloads)?;
    let archive = downloads.join(format!("{version}.archive"));
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
        ])
        .arg(&archive)
        .arg(&artifact.url)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| ServiceError::Update(format!("failed to run curl: {error}")))?;
    if !status.success() {
        return Err(ServiceError::Update("artifact download failed".to_string()));
    }
    verify_sha256(&archive, &artifact.sha256)?;
    let staging = release_dir.with_extension("installing");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    extract_archive(&archive, &staging, &artifact.url)?;
    if let Some(parent) = release_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(staging, release_dir)?;
    let _ = std::fs::remove_file(archive);
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> ServiceResult<()> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(ServiceError::Update(
            "artifact SHA-256 mismatch".to_string(),
        ))
    }
}

fn extract_archive(archive: &Path, destination: &Path, url: &str) -> ServiceResult<()> {
    let zip = url.to_ascii_lowercase().ends_with(".zip");
    let (program, list_args, extract_args): (&str, Vec<&str>, Vec<&str>) = if zip {
        ("unzip", vec!["-Z1"], vec!["-q"])
    } else {
        ("tar", vec!["-tf"], vec!["-xf"])
    };
    let listing = Command::new(program)
        .args(list_args)
        .arg(archive)
        .output()
        .map_err(|error| ServiceError::Update(format!("failed to inspect archive: {error}")))?;
    if !listing.status.success() {
        return Err(ServiceError::Update(
            "could not inspect update archive".to_string(),
        ));
    }
    for entry in String::from_utf8_lossy(&listing.stdout).lines() {
        if !safe_relative_path(Path::new(entry)) {
            return Err(ServiceError::Update(
                "update archive contains an unsafe path".to_string(),
            ));
        }
    }
    let mut command = Command::new(program);
    command.args(extract_args).arg(archive);
    if zip {
        command.arg("-d").arg(destination);
    } else {
        command.arg("-C").arg(destination);
    }
    let status = command
        .status()
        .map_err(|error| ServiceError::Update(format!("failed to extract archive: {error}")))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| ServiceError::Update("update archive extraction failed".to_string()))
}

pub(crate) fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}
