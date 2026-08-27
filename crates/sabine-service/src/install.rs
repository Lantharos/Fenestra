use crate::{
    PrepareProgress, PrepareStage, ServiceError, ServiceResult, SystemReleaseManifest,
    registry::replace_file, service_data_dir, verify_system_release,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

mod artifacts;

use artifacts::{
    copy_directory, download_file, extract_system_archive, fetch_system_manifest,
    sabine_host_relative_path, service_binary_name, service_daemon_binary_name, system_asset_name,
    verify_sha256, which,
};

const SERVICE_REPO: &str = "Lantharos/Sabine";
const SYSTEM_UPDATE_PUBLIC_KEYS: &str = include_str!("../update-public-key.txt");

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SystemInstallationState {
    schema: u32,
    active: String,
    previous: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StagedSystemUpdate {
    pub version: String,
    pub service: PathBuf,
    pub previous_service: Option<PathBuf>,
}

pub fn cached_service_path() -> PathBuf {
    current_installation()
        .map(|(_, path)| path.join(service_binary_name()))
        .unwrap_or_else(|| {
            versions_dir()
                .join(env!("CARGO_PKG_VERSION"))
                .join(service_binary_name())
        })
}

pub fn service_daemon_path(service: &Path) -> PathBuf {
    service.with_file_name(service_daemon_binary_name())
}

fn complete_service_at(path: PathBuf) -> Option<PathBuf> {
    (path.is_file() && service_daemon_path(&path).is_file()).then_some(path)
}

pub fn find_service_executable() -> Option<PathBuf> {
    if let Some(path) = configured_service() {
        return Some(path);
    }

    if let Some((_, directory)) = current_installation() {
        return complete_service_at(directory.join(service_binary_name()));
    }

    if let Some(path) = adjacent_service() {
        return Some(path);
    }

    if let Ok(path) = which(service_binary_name())
        && let Some(path) = complete_service_at(path)
    {
        return Some(path);
    }

    None
}

pub fn ensure_service_executable(
    mut on_progress: impl FnMut(PrepareProgress),
) -> ServiceResult<PathBuf> {
    if let Some(path) = configured_service() {
        return Ok(path);
    }
    if let Some((_, directory)) = current_installation() {
        return Ok(directory.join(service_binary_name()));
    }
    if let Some(path) = adjacent_service() {
        return seed_managed_install(&path);
    }
    if let Ok(path) = which(service_binary_name())
        && let Some(path) = complete_service_at(path)
    {
        return Ok(path);
    }

    on_progress(PrepareProgress {
        stage: PrepareStage::Service,
        message: "Downloading Sabine service".to_string(),
        fraction: Some(0.02),
    });

    let update = install_latest_system(false, &mut on_progress)?;
    let destination = update
        .map(|update| update.service)
        .or_else(|| complete_service_at(cached_service_path()))
        .ok_or_else(|| ServiceError::Update("Sabine system installation is incomplete".into()))?;
    on_progress(PrepareProgress {
        stage: PrepareStage::Service,
        message: "Sabine service ready".to_string(),
        fraction: Some(0.08),
    });
    Ok(destination)
}

fn configured_service() -> Option<PathBuf> {
    std::env::var_os("SABINE_SERVICE_PATH")
        .map(PathBuf::from)
        .and_then(complete_service_at)
}

fn adjacent_service() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    complete_service_at(current.parent()?.join(service_binary_name()))
}

fn seed_managed_install(service: &Path) -> ServiceResult<PathBuf> {
    let source_dir = service.parent().ok_or_else(|| {
        ServiceError::Update("bundled Sabine service has no parent directory".to_string())
    })?;
    let version = env!("CARGO_PKG_VERSION");
    let destination = versions_dir().join(version);
    let installed_service = destination.join(service_binary_name());
    if complete_service_at(installed_service.clone()).is_none()
        || !destination.join(sabine_host_relative_path()).is_file()
    {
        let staging = versions_dir().join(format!("{version}.installing"));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        for name in [service_binary_name(), service_daemon_binary_name()] {
            let source = source_dir.join(name);
            if !source.is_file() {
                return Err(ServiceError::Update(format!(
                    "offline Sabine system bundle is missing {name}"
                )));
            }
            let target = staging.join(name);
            fs::copy(source, &target)?;
            make_executable(&target)?;
        }
        if cfg!(target_os = "macos") {
            copy_directory(
                &source_dir.join("sabine-host.app"),
                &staging.join("sabine-host.app"),
            )?;
        } else {
            let name = sabine_host_relative_path();
            let source = source_dir.join(&name);
            if !source.is_file() {
                return Err(ServiceError::Update(format!(
                    "offline Sabine system bundle is missing {}",
                    name.display()
                )));
            }
            let target = staging.join(name);
            fs::copy(source, &target)?;
            make_executable(&target)?;
        }
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        fs::rename(staging, &destination)?;
    }
    write_installation_state(&SystemInstallationState {
        schema: 1,
        active: version.to_string(),
        previous: None,
    })?;
    Ok(installed_service)
}

pub fn stage_system_update() -> ServiceResult<Option<StagedSystemUpdate>> {
    if read_installation_state().is_none() {
        return Ok(None);
    }
    install_latest_system(true, &mut |_| {})
}

pub fn rollback_system_update(failed_version: &str) -> ServiceResult<Option<PathBuf>> {
    let Some(state) = read_installation_state() else {
        return Ok(None);
    };
    if state.active != failed_version {
        return Ok(complete_service_at(
            versions_dir()
                .join(&state.active)
                .join(service_binary_name()),
        ));
    }
    let Some(previous) = state.previous else {
        return Ok(None);
    };
    let service = versions_dir().join(&previous).join(service_binary_name());
    let Some(service) = complete_service_at(service) else {
        return Ok(None);
    };
    write_installation_state(&SystemInstallationState {
        schema: 1,
        active: previous,
        previous: None,
    })?;
    let failed = versions_dir().join(failed_version);
    if failed.is_dir() {
        let _ = fs::remove_dir_all(failed);
    }
    Ok(Some(service))
}

fn install_latest_system(
    require_newer: bool,
    on_progress: &mut impl FnMut(PrepareProgress),
) -> ServiceResult<Option<StagedSystemUpdate>> {
    let manifest = fetch_system_manifest()?;
    if !SYSTEM_UPDATE_PUBLIC_KEYS
        .lines()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .any(|key| verify_system_release(&manifest, key).is_ok())
    {
        return Err(ServiceError::Update(
            "Sabine release signature is not trusted".to_string(),
        ));
    }
    if manifest.schema != 1 {
        return Err(ServiceError::Update(format!(
            "unsupported Sabine release schema {}",
            manifest.schema
        )));
    }
    if manifest.version.trim().is_empty() {
        return Err(ServiceError::Update(
            "Sabine release version is missing".to_string(),
        ));
    }
    let previous = read_installation_state();
    if require_newer
        && previous
            .as_ref()
            .is_some_and(|state| !crate::types::version_is_newer(&manifest.version, &state.active))
    {
        return Ok(None);
    }
    let install_dir = versions_dir().join(&manifest.version);
    let destination = install_dir.join(service_binary_name());
    if complete_service_at(destination.clone()).is_none()
        || !install_dir.join(sabine_host_relative_path()).is_file()
    {
        install_system_archive(&manifest, &install_dir, on_progress)?;
    }
    let previous_version = previous
        .as_ref()
        .map(|state| state.active.clone())
        .filter(|version| version != &manifest.version);
    write_installation_state(&SystemInstallationState {
        schema: 1,
        active: manifest.version.clone(),
        previous: previous_version.clone(),
    })?;
    prune_system_versions(&manifest.version, previous_version.as_deref())?;
    Ok(Some(StagedSystemUpdate {
        version: manifest.version,
        service: destination,
        previous_service: previous_version
            .map(|version| versions_dir().join(version).join(service_binary_name()))
            .and_then(complete_service_at),
    }))
}

fn install_system_archive(
    manifest: &SystemReleaseManifest,
    install_dir: &Path,
    on_progress: &mut impl FnMut(PrepareProgress),
) -> ServiceResult<()> {
    let name = system_asset_name();
    let artifact = manifest
        .artifacts
        .get(&name)
        .ok_or_else(|| ServiceError::Update(format!("Sabine release has no {name} artifact")))?;
    if !artifact.url.starts_with("https://") {
        return Err(ServiceError::Update(
            "Sabine system artifact URL must use HTTPS".to_string(),
        ));
    }
    if artifact.sha256.len() != 64 || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ServiceError::Update(
            "Sabine system artifact SHA-256 is invalid".to_string(),
        ));
    }
    let downloads = service_data_dir().join("downloads/system");
    fs::create_dir_all(&downloads)?;
    let archive = downloads.join(format!("{}-{name}", manifest.version));
    download_file(&artifact.url, &archive, on_progress)?;
    verify_sha256(&archive, &artifact.sha256)?;
    let actual_size = fs::metadata(&archive)?.len();
    if actual_size != artifact.size {
        return Err(ServiceError::Update(format!(
            "Sabine system bundle size mismatch: expected {}, got {actual_size}",
            artifact.size
        )));
    }

    let staging = versions_dir().join(format!("{}.installing", manifest.version));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    extract_system_archive(&archive, &staging)?;
    for name in [
        PathBuf::from(service_binary_name()),
        PathBuf::from(service_daemon_binary_name()),
        sabine_host_relative_path(),
    ] {
        let source = staging.join(&name);
        if !source.is_file() {
            return Err(ServiceError::Update(format!(
                "Sabine system bundle is missing {}",
                name.display()
            )));
        }
        make_executable(&source)?;
    }
    if install_dir.exists() {
        fs::remove_dir_all(install_dir)?;
    }
    fs::rename(&staging, install_dir)?;
    let _ = fs::remove_file(archive);
    Ok(())
}

fn versions_dir() -> PathBuf {
    service_data_dir().join("bin/versions")
}

fn installation_state_path() -> PathBuf {
    service_data_dir().join("bin/current.json")
}

fn read_installation_state() -> Option<SystemInstallationState> {
    let state = serde_json::from_slice::<SystemInstallationState>(
        &fs::read(installation_state_path()).ok()?,
    )
    .ok()?;
    (state.schema == 1).then_some(state)
}

fn current_installation() -> Option<(String, PathBuf)> {
    let state = read_installation_state()?;
    let path = versions_dir().join(&state.active);
    complete_service_at(path.join(service_binary_name()))?;
    Some((state.active, path))
}

fn write_installation_state(state: &SystemInstallationState) -> ServiceResult<()> {
    let path = installation_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("new");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(state).expect("system state is serializable"))?;
    file.sync_all()?;
    replace_file(&temporary, &path)?;
    Ok(())
}

fn prune_system_versions(active: &str, previous: Option<&str>) -> ServiceResult<()> {
    let base = versions_dir();
    if !base.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(base)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if path.is_dir()
            && name != active
            && Some(name) != previous
            && !name.ends_with(".installing")
        {
            let _ = fs::remove_dir_all(path);
        }
    }
    Ok(())
}

fn make_executable(path: &Path) -> ServiceResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
