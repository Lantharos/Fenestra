use sabine_runtime::{
    prune_user_runtimes, quarantine_user_runtime, resolve_runtime,
    update_user_runtime_with_progress,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::registry::{RegistryLock, SabineService, replace_file};
use crate::types::{
    AppArtifact, AppArtifactKind, AppInstallMode, AppReleaseManifest, AppUpdateStatus,
    MaintenanceReport, PendingAppUpdate, ServiceError, ServiceResult, UpdatePolicy, is_https_url,
    unix_timestamp, update_artifact_target, version_is_newer,
};
use crate::verify_app_release;

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

fn install_archive(
    root: &Path,
    id: &str,
    version: &str,
    artifact: &AppArtifact,
    release_dir: &Path,
) -> ServiceResult<()> {
    let downloads = root.join("downloads").join(id);
    std::fs::create_dir_all(&downloads)?;
    let archive = downloads.join(format!("{version}.archive"));
    download_artifact(&artifact.url, &archive)?;
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

fn download_artifact(url: &str, destination: &Path) -> ServiceResult<()> {
    let temporary = destination.with_extension("download");
    let response = ureq::get(url)
        .call()
        .map_err(|error| ServiceError::Update(format!("artifact download failed: {error}")))?;
    let (_, body) = response.into_parts();
    let mut reader = body.into_reader();
    let mut file = File::create(&temporary)?;
    std::io::copy(&mut reader, &mut file)?;
    file.flush()?;
    file.sync_all()?;
    replace_file(&temporary, destination)?;
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> ServiceResult<()> {
    let mut file = File::open(path)?;
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
    let (program, list_args, extract_args): (&str, &[&str], &[&str]) = if zip {
        ("unzip", &["-Z1"], &["-q"])
    } else {
        ("tar", &["-tf"], &["-xf"])
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

fn run_package_installer(
    update: &PendingAppUpdate,
    install_target: Option<&Path>,
) -> ServiceResult<()> {
    let mut command = match update.kind {
        AppArtifactKind::Deb => {
            elevated_command("apt-get", &["install", "--yes"], &update.artifact)?
        }
        AppArtifactKind::Rpm => {
            if command_exists("dnf") {
                elevated_command("dnf", &["install", "--assumeyes"], &update.artifact)?
            } else {
                elevated_command("rpm", &["-U"], &update.artifact)?
            }
        }
        AppArtifactKind::Msi => {
            let mut command = Command::new("msiexec");
            command
                .arg("/i")
                .arg(&update.artifact)
                .args(["/passive", "/norestart"]);
            command
        }
        AppArtifactKind::Exe => Command::new(&update.artifact),
        AppArtifactKind::Dmg => return install_dmg(update, install_target),
        AppArtifactKind::AppImage => return install_appimage(update, install_target),
        AppArtifactKind::Archive => {
            return Err(ServiceError::Update(
                "archive updates do not use a package installer".to_string(),
            ));
        }
    };
    let status = command
        .stdin(Stdio::null())
        .status()
        .map_err(|error| ServiceError::Update(format!("failed to start installer: {error}")))?;
    let success = status.success()
        || (matches!(update.kind, AppArtifactKind::Msi)
            && status.code().is_some_and(|code| code == 3010));
    success
        .then_some(())
        .ok_or_else(|| ServiceError::Update(format!("installer exited with {status}")))
}

fn install_appimage(update: &PendingAppUpdate, target: Option<&Path>) -> ServiceResult<()> {
    if !cfg!(target_os = "linux") {
        return Err(ServiceError::Update(
            "AppImage updates are only supported on Linux".to_string(),
        ));
    }
    let target = target.ok_or_else(|| {
        ServiceError::Update("AppImage update is missing its installation path".to_string())
    })?;
    let temporary = target.with_extension("sabine-update");
    std::fs::copy(&update.artifact, &temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&temporary)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&temporary, permissions)?;
    }
    replace_file(&temporary, target)?;
    Ok(())
}

fn install_dmg(update: &PendingAppUpdate, target: Option<&Path>) -> ServiceResult<()> {
    if !cfg!(target_os = "macos") {
        return Err(ServiceError::Update(
            "DMG updates are only supported on macOS".to_string(),
        ));
    }
    let executable = target.ok_or_else(|| {
        ServiceError::Update("DMG update is missing its application path".to_string())
    })?;
    let app = executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .ok_or_else(|| ServiceError::Update("could not locate installed macOS app".to_string()))?;
    let mount = update.artifact.with_extension("mount");
    if mount.exists() {
        let _ = std::fs::remove_dir_all(&mount);
    }
    std::fs::create_dir_all(&mount)?;
    let attach = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount)
        .arg(&update.artifact)
        .status()?;
    if !attach.success() {
        return Err(ServiceError::Update(
            "could not mount app update".to_string(),
        ));
    }
    let result = (|| {
        let source = std::fs::read_dir(&mount)?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "app"))
            .ok_or_else(|| {
                ServiceError::Update("DMG contains no application bundle".to_string())
            })?;
        let source = shell_single_quote(&source.display().to_string());
        let destination = shell_single_quote(&app.display().to_string());
        let staged = shell_single_quote(&format!("{}.sabine-new", app.display()));
        let backup = shell_single_quote(&format!("{}.sabine-old", app.display()));
        let script = format!(
            "/bin/rm -rf {staged} {backup} && /usr/bin/ditto {source} {staged} && /bin/mv {destination} {backup} && (/bin/mv {staged} {destination} && /bin/rm -rf {backup} || (/bin/mv {backup} {destination}; exit 1))"
        );
        let status = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "do shell script {} with administrator privileges",
                apple_script_string(&script)
            ))
            .status()?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| ServiceError::Update("macOS app installation was cancelled".to_string()))
    })();
    let _ = Command::new("hdiutil").arg("detach").arg(&mount).status();
    let _ = std::fs::remove_dir_all(mount);
    result
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn apple_script_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn elevated_command(program: &str, args: &[&str], artifact: &Path) -> ServiceResult<Command> {
    if !cfg!(target_os = "linux") {
        return Err(ServiceError::Update(
            "this package installer is unsupported on the current platform".to_string(),
        ));
    }
    let mut command = Command::new("pkexec");
    command.arg(program).args(args).arg(artifact);
    Ok(command)
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

fn artifact_extension(kind: AppArtifactKind) -> &'static str {
    match kind {
        AppArtifactKind::Archive => "archive",
        AppArtifactKind::Deb => "deb",
        AppArtifactKind::Rpm => "rpm",
        AppArtifactKind::Msi => "msi",
        AppArtifactKind::Exe => "exe",
        AppArtifactKind::Dmg => "dmg",
        AppArtifactKind::AppImage => "AppImage",
    }
}

fn pending_path(root: &Path, id: &str) -> PathBuf {
    root.join("apps").join(id).join("pending-update.json")
}

fn write_json_atomic(path: &Path, value: &impl serde::Serialize) -> ServiceResult<()> {
    let temporary = path.with_extension("new");
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value).expect("pending update is serializable"))?;
    file.sync_all()?;
    replace_file(&temporary, path)?;
    Ok(())
}

pub(crate) fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}
