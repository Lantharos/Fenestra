use sabine_runtime::{
    prune_user_runtimes, quarantine_user_runtime, resolve_runtime,
    update_user_runtime_with_progress,
};
use std::path::{Path, PathBuf};

use crate::registry::{RegistryLock, SabineService};
use crate::types::{
    AppArtifact, AppArtifactKind, AppInstallMode, AppReleaseManifest, AppUpdateStatus,
    MaintenanceReport, PendingAppUpdate, ServiceError, ServiceResult, UpdatePolicy, is_https_url,
    unix_timestamp, update_artifact_target, version_is_newer,
};
use crate::verify_app_release;

mod installers;

pub(crate) use installers::safe_relative_path;
use installers::{
    artifact_extension, download_artifact, install_archive, pending_path, run_package_installer,
    verify_sha256, write_json_atomic,
};

impl SabineService {
    pub fn maintain(&self) -> ServiceResult<MaintenanceReport> {
        retry_quarantined_runtimes()?;
        let mut runtime = update_user_runtime_with_progress(&self.runtime, |_| {})?;
        let host = sabine_host::available_host(runtime.location.path()).ok_or_else(|| {
            ServiceError::Update(
                "installed Sabine host is unavailable for runtime validation".into(),
            )
        })?;
        if let Err(error) = sabine_host::smoke_test_runtime(&host, runtime.location.path()) {
            quarantine_user_runtime(
                &runtime,
                &format!(
                    "probe={}\n{error}",
                    sabine_host::runtime_probe_fingerprint()
                ),
            )?;
            runtime = resolve_runtime(&self.runtime)?;
        }
        let pruned_runtimes = prune_user_runtimes(2)?;
        let apps = self.apps()?;
        let mut updated_apps = Vec::new();
        let mut pending_apps = Vec::new();
        let mut update_failures = Vec::new();
        for app in &apps {
            let Some(update) = &app.manifest.update else {
                continue;
            };
            if update.policy != UpdatePolicy::Automatic {
                continue;
            }
            match self.update_app(&app.manifest.id) {
                Ok(AppUpdateStatus::Installed { .. }) => updated_apps.push(app.manifest.id.clone()),
                Ok(AppUpdateStatus::PendingApproval(_)) => {
                    pending_apps.push(app.manifest.id.clone())
                }
                Ok(AppUpdateStatus::Current | AppUpdateStatus::StoreManaged) => {}
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
            pending_apps,
            update_failures,
        })
    }

    pub fn update_app(&self, id: &str) -> ServiceResult<AppUpdateStatus> {
        let app = self.app(id)?;
        let update = app
            .manifest
            .update
            .as_ref()
            .ok_or_else(|| ServiceError::Update("app has no update source".to_string()))?;
        if update.install_mode == AppInstallMode::Store {
            return Ok(AppUpdateStatus::StoreManaged);
        }
        let manifest_url = update.source.manifest_url(&update.channel)?;
        let release = fetch_release(&manifest_url)?;
        verify_app_release(&release, &update.public_key)?;
        validate_release(&release, id, &update.channel)?;
        if !version_is_newer(&release.version, &app.manifest.version) {
            return Ok(AppUpdateStatus::Current);
        }
        let target = update_artifact_target(update.install_mode, update.package_kind);
        let artifact = release
            .artifacts
            .get(&target)
            .ok_or_else(|| ServiceError::Update(format!("release has no artifact for {target}")))?;
        validate_artifact(artifact)?;

        if update.install_mode == AppInstallMode::Package
            || artifact.kind != AppArtifactKind::Archive
        {
            let pending = self.stage_package_update(id, &release.version, artifact)?;
            return Ok(AppUpdateStatus::PendingApproval(pending));
        }

        let executable = artifact.executable.as_ref().ok_or_else(|| {
            ServiceError::Update("managed update artifact has no executable".to_string())
        })?;
        let release_dir = self
            .root
            .join("apps")
            .join(id)
            .join("releases")
            .join(&release.version);
        let installed_executable = release_dir.join(executable);
        if !installed_executable.is_file() {
            install_archive(&self.root, id, &release.version, artifact, &release_dir)?;
        }
        if !installed_executable.is_file() {
            return Err(ServiceError::Update(format!(
                "artifact did not contain {}",
                executable.display()
            )));
        }
        self.activate_managed_update(id, &release.version, installed_executable)?;
        Ok(AppUpdateStatus::Installed {
            version: release.version,
        })
    }

    pub fn pending_app_update(&self, id: &str) -> ServiceResult<Option<PendingAppUpdate>> {
        let path = pending_path(&self.root, id);
        if !path.is_file() {
            return Ok(None);
        }
        serde_json::from_slice(&std::fs::read(&path)?)
            .map(Some)
            .map_err(|error| ServiceError::Decode {
                path,
                source: error,
            })
    }

    pub fn apply_pending_app_update(
        &self,
        id: &str,
        install_target: Option<&Path>,
    ) -> ServiceResult<bool> {
        let Some(pending) = self.pending_app_update(id)? else {
            return Ok(false);
        };
        verify_sha256(&pending.artifact, &pending.sha256)?;
        run_package_installer(&pending, install_target)?;
        std::fs::remove_file(pending_path(&self.root, id))?;
        Ok(true)
    }

    fn stage_package_update(
        &self,
        id: &str,
        version: &str,
        artifact: &AppArtifact,
    ) -> ServiceResult<PendingAppUpdate> {
        let downloads = self.root.join("downloads").join(id);
        std::fs::create_dir_all(&downloads)?;
        let path = downloads.join(format!("{version}.{}", artifact_extension(artifact.kind)));
        if path.is_file() && verify_sha256(&path, &artifact.sha256).is_err() {
            std::fs::remove_file(&path)?;
        }
        if !path.is_file() {
            download_artifact(&artifact.url, &path)?;
        }
        verify_sha256(&path, &artifact.sha256)?;
        let pending = PendingAppUpdate {
            app_id: id.to_string(),
            version: version.to_string(),
            artifact: path,
            sha256: artifact.sha256.clone(),
            kind: artifact.kind,
            requires_elevation: artifact.kind.requires_elevation(),
        };
        let pending_path = pending_path(&self.root, id);
        if let Some(parent) = pending_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_json_atomic(&pending_path, &pending)?;
        Ok(pending)
    }

    fn activate_managed_update(
        &self,
        id: &str,
        version: &str,
        executable: PathBuf,
    ) -> ServiceResult<()> {
        let _lock = RegistryLock::acquire(&self.root)?;
        let mut registry = self.load_registry()?;
        let registered = registry
            .apps
            .get_mut(id)
            .ok_or_else(|| ServiceError::AppNotFound(id.to_string()))?;
        if !version_is_newer(version, &registered.manifest.version) {
            return Ok(());
        }
        registered.manifest.version = version.to_string();
        registered.manifest.executable = executable;
        registered.updated_at = unix_timestamp();
        self.save_registry(&registry)
    }
}

pub fn retry_quarantined_runtimes() -> ServiceResult<()> {
    let root = sabine_runtime::user_runtime_path();
    let current = format!("probe={}", sabine_host::runtime_probe_fingerprint());
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let marker = entry.path().join(".sabine-unusable");
        let Ok(reason) = std::fs::read_to_string(&marker) else {
            continue;
        };
        if !quarantine_belongs_to_host(&reason, &current) {
            std::fs::remove_file(marker)?;
        }
    }
    Ok(())
}

pub(super) fn quarantine_belongs_to_host(reason: &str, current: &str) -> bool {
    reason.lines().next() == Some(current)
}

fn fetch_release(url: &str) -> ServiceResult<AppReleaseManifest> {
    if !is_https_url(url) {
        return Err(ServiceError::Update(
            "release manifest URL must use HTTPS".to_string(),
        ));
    }
    let mut response = ureq::get(url)
        .call()
        .map_err(|error| ServiceError::Update(format!("release request failed: {error}")))?;
    let body = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| ServiceError::Update(format!("release response failed: {error}")))?;
    serde_json::from_slice(&body)
        .map_err(|error| ServiceError::Update(format!("invalid release manifest: {error}")))
}

fn validate_release(release: &AppReleaseManifest, id: &str, channel: &str) -> ServiceResult<()> {
    if release.schema != 1 {
        return Err(ServiceError::Update(format!(
            "unsupported release manifest schema {}",
            release.schema
        )));
    }
    if release.app_id != id {
        return Err(ServiceError::Update(format!(
            "release belongs to {} instead of {id}",
            release.app_id
        )));
    }
    if release.channel != channel {
        return Err(ServiceError::Update(format!(
            "release channel {} does not match {channel}",
            release.channel
        )));
    }
    Ok(())
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
    if artifact
        .executable
        .as_deref()
        .is_some_and(|path| !safe_relative_path(path))
    {
        return Err(ServiceError::Update(
            "artifact executable path is unsafe".to_string(),
        ));
    }
    Ok(())
}
