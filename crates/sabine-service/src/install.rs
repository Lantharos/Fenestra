use crate::{
    PrepareProgress, PrepareStage, SabineVersion, ServiceError, ServiceResult, SystemCompatibility,
    SystemReleaseManifest, service_data_dir, verify_system_release,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

mod artifacts;
mod state;

use artifacts::{
    copy_directory, download_file, extract_system_archive, fetch_system_manifest,
    sabine_host_relative_path, service_binary_name, service_daemon_binary_name, system_asset_name,
    verify_sha256, which,
};
use state::{
    SystemInstallationState, clear_system_failure, compatibility_for_version, current_installation,
    finalize_system_update, normalized_state_compatibility, prune_system_versions,
    read_installation_state, record_system_failure, system_update_is_backed_off, versions_dir,
    write_installation_state,
};

const SERVICE_REPO: &str = "Lantharos/Sabine";
const SYSTEM_UPDATE_PUBLIC_KEYS: &str = include_str!("../update-public-key.txt");

#[derive(Clone, Debug, PartialEq, Eq)]
enum SystemUpdateMode {
    Required(SabineVersion),
    Repair {
        required: SabineVersion,
        release_version: String,
    },
    Routine,
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
        .unwrap_or_else(|| service_path_for_version(crate::SABINE_VERSION))
}

pub(crate) fn service_path_for_version(version: &str) -> PathBuf {
    versions_dir().join(version).join(service_binary_name())
}

pub fn service_daemon_path(service: &Path) -> PathBuf {
    service.with_file_name(service_daemon_binary_name())
}

fn complete_service_at(path: PathBuf) -> Option<PathBuf> {
    (path.is_file() && service_daemon_path(&path).is_file()).then_some(path)
}

fn complete_managed_system_at(directory: &Path) -> Option<PathBuf> {
    directory
        .join(sabine_host_relative_path())
        .is_file()
        .then_some(())?;
    complete_service_at(directory.join(service_binary_name()))
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
    if let Some((version, directory)) = current_installation() {
        let current = directory.join(service_binary_name());
        if !managed_system_is_older(&version) {
            return Ok(current);
        }
        on_progress(PrepareProgress {
            stage: PrepareStage::Service,
            message: "Updating Sabine service".to_string(),
            fraction: Some(0.02),
        });
        if let Some(path) = adjacent_service() {
            return seed_managed_install(&path);
        }
        if let Some(update) = install_latest_system(
            SystemUpdateMode::Required(SabineVersion::current()),
            &mut on_progress,
        )? {
            return Ok(update.service);
        }
        return Ok(current);
    }
    if let Some(state) = read_installation_state() {
        on_progress(PrepareProgress {
            stage: PrepareStage::Service,
            message: "Repairing Sabine service".to_string(),
            fraction: Some(0.02),
        });
        if managed_system_is_older(&state.active) {
            if let Some(path) = adjacent_service() {
                return seed_managed_install(&path);
            }
            if let Some(update) = install_latest_system(
                SystemUpdateMode::Required(SabineVersion::current()),
                &mut on_progress,
            )? {
                return Ok(update.service);
            }
        } else {
            let installed = SabineVersion::parse(&state.active).ok_or_else(|| {
                ServiceError::Update(format!(
                    "installed Sabine version {} is invalid",
                    state.active
                ))
            })?;
            if installed == SabineVersion::current()
                && let Some(path) = adjacent_service()
            {
                return seed_managed_install(&path);
            }
            if let Some(update) = install_latest_system(
                SystemUpdateMode::Repair {
                    required: installed,
                    release_version: state.active,
                },
                &mut on_progress,
            )? {
                return Ok(update.service);
            }
        }
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

    let update = install_latest_system(
        SystemUpdateMode::Required(SabineVersion::current()),
        &mut on_progress,
    )?;
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

fn managed_system_is_older(installed: &str) -> bool {
    crate::types::version_is_newer(crate::SABINE_VERSION, installed)
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
    let version = crate::SABINE_VERSION;
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
    let previous_state = read_installation_state();
    let previous = previous_state
        .as_ref()
        .map(|state| state.active.clone())
        .filter(|active| active != version);
    let previous_compatibility = previous_state
        .as_ref()
        .filter(|state| state.active != version)
        .map(normalized_state_compatibility);
    write_installation_state(&SystemInstallationState {
        schema: 1,
        active: version.to_string(),
        previous,
        compatibility: SystemCompatibility::current(),
        previous_compatibility,
    })?;
    Ok(installed_service)
}

pub fn stage_system_update() -> ServiceResult<Option<StagedSystemUpdate>> {
    if read_installation_state().is_none() {
        return Ok(None);
    }
    install_latest_system(SystemUpdateMode::Routine, &mut |_| {})
}

pub(crate) fn stage_required_system_update(
    required: SabineVersion,
) -> ServiceResult<Option<StagedSystemUpdate>> {
    install_latest_system(SystemUpdateMode::Required(required), &mut |_| {})
}

pub(crate) fn install_required_system_update(required: SabineVersion) -> ServiceResult<()> {
    let Some(_) = stage_required_system_update(required)? else {
        return Err(ServiceError::Update(format!(
            "Sabine {} is not available",
            required.label()
        )));
    };
    crate::ensure_daemon_running()?;
    let installed = installed_system_compatibility();
    if installed.accepts(required) {
        Ok(())
    } else {
        Err(ServiceError::Update(format!(
            "Sabine {} did not become active",
            required.label()
        )))
    }
}

pub fn repair_system_installation() -> ServiceResult<PathBuf> {
    let release_version = read_installation_state()
        .map(|state| state.active)
        .unwrap_or_else(|| crate::SABINE_VERSION.to_string());
    let required = SabineVersion::parse(&release_version).ok_or_else(|| {
        ServiceError::Update(format!(
            "installed Sabine version {release_version} is invalid"
        ))
    })?;
    install_latest_system(
        SystemUpdateMode::Repair {
            required,
            release_version,
        },
        &mut |_| {},
    )?
    .map(|update| update.service)
    .or_else(|| complete_service_at(cached_service_path()))
    .ok_or_else(|| ServiceError::Update("Sabine system repair did not produce a service".into()))
}

pub fn rollback_system_update(failed_version: &str) -> ServiceResult<Option<PathBuf>> {
    let Some(state) = read_installation_state() else {
        return Ok(None);
    };
    if state.active != failed_version {
        return Ok(complete_managed_system_at(
            &versions_dir().join(&state.active),
        ));
    }
    let Some(previous) = state.previous else {
        return Ok(None);
    };
    let directory = versions_dir().join(&previous);
    let Some(service) = complete_managed_system_at(&directory) else {
        return Ok(None);
    };
    let compatibility = state
        .previous_compatibility
        .unwrap_or_else(|| compatibility_for_version(&previous));
    write_installation_state(&SystemInstallationState {
        schema: 1,
        active: previous,
        previous: None,
        compatibility,
        previous_compatibility: None,
    })?;
    record_system_failure(failed_version)?;
    let failed = versions_dir().join(failed_version);
    if failed.is_dir() {
        let _ = fs::remove_dir_all(failed);
    }
    Ok(Some(service))
}

fn install_latest_system(
    mode: SystemUpdateMode,
    on_progress: &mut impl FnMut(PrepareProgress),
) -> ServiceResult<Option<StagedSystemUpdate>> {
    let requested_version = match &mode {
        SystemUpdateMode::Required(required) => Some(required.label()),
        SystemUpdateMode::Repair {
            release_version, ..
        } => Some(release_version.clone()),
        SystemUpdateMode::Routine => None,
    };
    let manifest = fetch_system_manifest(requested_version.as_deref())?;
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
    let compatibility = normalized_release_compatibility(&manifest)?;
    let required = match &mode {
        SystemUpdateMode::Required(required) | SystemUpdateMode::Repair { required, .. } => {
            Some(*required)
        }
        SystemUpdateMode::Routine => None,
    };
    if let Some(required) = required
        && (compatibility.major != required.major || compatibility.build < required.build)
    {
        return Err(ServiceError::Update(format!(
            "latest Sabine system {} cannot run an app requiring Sabine {}",
            manifest.version,
            required.label()
        )));
    }
    if mode == SystemUpdateMode::Routine {
        if !crate::release_is_soaked(&manifest.published_at) {
            return Ok(None);
        }
        if system_update_is_backed_off(&manifest.version) {
            return Ok(None);
        }
    }
    let previous = read_installation_state();
    if !matches!(&mode, SystemUpdateMode::Repair { .. })
        && previous
            .as_ref()
            .is_some_and(|state| !crate::types::version_is_newer(&manifest.version, &state.active))
    {
        return Ok(None);
    }
    let install_dir = versions_dir().join(&manifest.version);
    let destination = install_dir.join(service_binary_name());
    if (matches!(&mode, SystemUpdateMode::Repair { .. })
        || complete_service_at(destination.clone()).is_none()
        || !install_dir.join(sabine_host_relative_path()).is_file())
        && let Err(error) = install_system_archive(&manifest, &install_dir, on_progress)
    {
        record_system_failure(&manifest.version)?;
        return Err(error);
    }
    let repairing_active = previous
        .as_ref()
        .is_some_and(|state| state.active == manifest.version);
    let previous_version = if repairing_active {
        previous.as_ref().and_then(|state| state.previous.clone())
    } else {
        previous.as_ref().map(|state| state.active.clone())
    };
    let previous_compatibility = if repairing_active {
        previous
            .as_ref()
            .and_then(|state| state.previous_compatibility)
    } else {
        previous.as_ref().map(normalized_state_compatibility)
    };
    write_installation_state(&SystemInstallationState {
        schema: 1,
        active: manifest.version.clone(),
        previous: previous_version.clone(),
        compatibility,
        previous_compatibility,
    })?;
    prune_system_versions(&manifest.version, previous_version.as_deref())?;
    Ok(Some(StagedSystemUpdate {
        version: manifest.version,
        service: destination,
        previous_service: previous_version
            .map(|version| versions_dir().join(version))
            .and_then(|directory| complete_managed_system_at(&directory)),
    }))
}

pub(crate) fn mark_system_update_healthy(version: &str) {
    let _ = clear_system_failure(version);
    let _ = finalize_system_update(version);
}

pub fn installed_system_compatibility() -> SystemCompatibility {
    read_installation_state()
        .map(|state| normalized_state_compatibility(&state))
        .unwrap_or_else(SystemCompatibility::current)
}

fn normalized_release_compatibility(
    manifest: &SystemReleaseManifest,
) -> ServiceResult<SystemCompatibility> {
    let version = SabineVersion::parse(&manifest.version).ok_or_else(|| {
        ServiceError::Update(format!(
            "invalid Sabine release version {}",
            manifest.version
        ))
    })?;
    let compatibility = if manifest.compatibility.build == 0 {
        SystemCompatibility {
            major: version.major,
            build: version.build,
            minimum_app_build: 1,
        }
    } else {
        manifest.compatibility
    };
    if compatibility.major != version.major
        || compatibility.build != version.build
        || compatibility.minimum_app_build > compatibility.build
    {
        return Err(ServiceError::Update(
            "Sabine release compatibility is inconsistent with its version".to_string(),
        ));
    }
    Ok(compatibility)
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

#[cfg(test)]
mod tests {
    use super::managed_system_is_older;

    #[test]
    fn managed_system_upgrade_only_replaces_older_versions() {
        assert!(managed_system_is_older("0.0.1"));
        assert!(!managed_system_is_older(crate::SABINE_VERSION));
        assert!(!managed_system_is_older("999.0.0"));
    }
}
