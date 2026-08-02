use mullion_runtime::{
    RuntimeConfig, RuntimeInfo, RuntimeInstallProgress, ensure_runtime,
    install_user_runtime_with_progress, prune_user_runtimes, resolve_runtime,
    update_user_runtime_with_progress,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const REGISTRY_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid app manifest: {0}")]
    InvalidManifest(String),
    #[error("app `{0}` is not registered")]
    AppNotFound(String),
    #[error("runtime operation failed: {0}")]
    Runtime(#[from] mullion_runtime::RuntimeError),
    #[error("app update failed: {0}")]
    Update(String),
    #[error("could not decode {path}: {source}")]
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ServiceResult<T> = Result<T, ServiceError>;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePolicy {
    Disabled,
    Notify,
    #[default]
    Automatic,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppUpdateConfig {
    pub manifest_url: String,
    #[serde(default = "stable_channel")]
    pub channel: String,
    #[serde(default)]
    pub policy: UpdatePolicy,
}

fn stable_channel() -> String {
    "stable".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub update: Option<AppUpdateConfig>,
}

impl AppManifest {
    pub fn validate(&self) -> ServiceResult<()> {
        if !valid_app_id(&self.id) {
            return Err(ServiceError::InvalidManifest(
                "id must contain only lowercase letters, digits, dots, and hyphens".to_string(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(ServiceError::InvalidManifest(
                "name is required".to_string(),
            ));
        }
        if self.version.trim().is_empty() {
            return Err(ServiceError::InvalidManifest(
                "version is required".to_string(),
            ));
        }
        if self.executable.as_os_str().is_empty() {
            return Err(ServiceError::InvalidManifest(
                "executable is required".to_string(),
            ));
        }
        if self
            .update
            .as_ref()
            .is_some_and(|update| !is_https_url(&update.manifest_url))
        {
            return Err(ServiceError::InvalidManifest(
                "update manifests must use HTTPS".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RegisteredApp {
    #[serde(flatten)]
    pub manifest: AppManifest,
    pub registered_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppReleaseManifest {
    pub version: String,
    #[serde(default = "stable_channel")]
    pub channel: String,
    pub artifacts: BTreeMap<String, AppArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppArtifact {
    pub url: String,
    pub sha256: String,
    pub executable: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RegistryFile {
    version: u32,
    #[serde(default)]
    apps: BTreeMap<String, RegisteredApp>,
}

impl Default for RegistryFile {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            apps: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MullionService {
    root: PathBuf,
    runtime: RuntimeConfig,
}

impl Default for MullionService {
    fn default() -> Self {
        Self::new(service_data_dir())
    }
}

impl MullionService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            runtime: RuntimeConfig::default(),
        }
    }

    pub fn with_runtime(mut self, runtime: RuntimeConfig) -> Self {
        self.runtime = runtime;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn register(&self, manifest: AppManifest) -> ServiceResult<RegisteredApp> {
        manifest.validate()?;
        let _lock = RegistryLock::acquire(&self.root)?;
        let mut registry = self.load_registry()?;
        let now = unix_timestamp();
        let registered_at = registry
            .apps
            .get(&manifest.id)
            .map(|app| app.registered_at)
            .unwrap_or(now);
        let app = RegisteredApp {
            manifest,
            registered_at,
            updated_at: now,
        };
        registry.apps.insert(app.manifest.id.clone(), app.clone());
        self.save_registry(&registry)?;
        Ok(app)
    }

    pub fn unregister(&self, id: &str) -> ServiceResult<RegisteredApp> {
        let _lock = RegistryLock::acquire(&self.root)?;
        let mut registry = self.load_registry()?;
        let app = registry
            .apps
            .remove(id)
            .ok_or_else(|| ServiceError::AppNotFound(id.to_string()))?;
        self.save_registry(&registry)?;
        Ok(app)
    }

    pub fn apps(&self) -> ServiceResult<Vec<RegisteredApp>> {
        let _lock = RegistryLock::acquire(&self.root)?;
        Ok(self.load_registry()?.apps.into_values().collect())
    }

    pub fn app(&self, id: &str) -> ServiceResult<RegisteredApp> {
        let _lock = RegistryLock::acquire(&self.root)?;
        self.load_registry()?
            .apps
            .remove(id)
            .ok_or_else(|| ServiceError::AppNotFound(id.to_string()))
    }

    pub fn runtime(&self) -> ServiceResult<RuntimeInfo> {
        resolve_runtime(&self.runtime).map_err(Into::into)
    }

    pub fn ensure_runtime(&self) -> ServiceResult<RuntimeInfo> {
        ensure_runtime(&self.runtime).map_err(Into::into)
    }

    pub fn ensure_runtime_with_progress(
        &self,
        mut progress: impl FnMut(RuntimeInstallProgress),
    ) -> ServiceResult<RuntimeInfo> {
        match resolve_runtime(&self.runtime) {
            Ok(runtime) => Ok(runtime),
            Err(_) => {
                install_user_runtime_with_progress(&self.runtime, &mut progress).map_err(Into::into)
            }
        }
    }

    pub fn maintain(&self) -> ServiceResult<MaintenanceReport> {
        let runtime = update_user_runtime_with_progress(&self.runtime, |_| {})?;
        let pruned_runtimes = prune_user_runtimes(&self.runtime, 2)?;
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

    fn registry_path(&self) -> PathBuf {
        self.root.join("apps.json")
    }

    fn load_registry(&self) -> ServiceResult<RegistryFile> {
        let path = self.registry_path();
        let backup = self.root.join("apps.json.bak");
        let source = if path.is_file() {
            path
        } else if backup.is_file() {
            backup
        } else {
            return Ok(RegistryFile::default());
        };
        let bytes = std::fs::read(&source)?;
        let registry = serde_json::from_slice::<RegistryFile>(&bytes).map_err(|error| {
            ServiceError::Decode {
                path: source,
                source: error,
            }
        })?;
        Ok(registry)
    }

    fn save_registry(&self, registry: &RegistryFile) -> ServiceResult<()> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.registry_path();
        let temporary = self.root.join("apps.json.new");
        let bytes = serde_json::to_vec_pretty(registry).expect("registry is serializable");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        replace_registry_file(&temporary, &path)?;
        Ok(())
    }
}

struct RegistryLock {
    path: PathBuf,
}

impl RegistryLock {
    fn acquire(root: &Path) -> ServiceResult<Self> {
        std::fs::create_dir_all(root)?;
        let path = root.join("apps.lock");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(60));
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(ServiceError::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "timed out waiting for the app registry",
                        )));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn replace_registry_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(not(target_os = "windows"))]
    return std::fs::rename(temporary, destination);

    #[cfg(target_os = "windows")]
    {
        let backup = destination.with_extension("json.bak");
        let _ = std::fs::remove_file(&backup);
        if destination.is_file() {
            std::fs::rename(destination, &backup)?;
        }
        match std::fs::rename(temporary, destination) {
            Ok(()) => {
                let _ = std::fs::remove_file(backup);
                Ok(())
            }
            Err(error) => {
                if backup.is_file() {
                    let _ = std::fs::rename(backup, destination);
                }
                Err(error)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct MaintenanceReport {
    pub runtime: RuntimeInfo,
    pub pruned_runtimes: usize,
    pub registered_apps: usize,
    pub automatic_updates: usize,
    pub updated_apps: Vec<String>,
    pub update_failures: Vec<String>,
}

pub fn service_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("Mullion");
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path)
            .join("Library")
            .join("Application Support")
            .join("Mullion");
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("mullion");
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join(".local/share/mullion")
}

pub fn default_maintenance_interval() -> Duration {
    Duration::from_secs(6 * 60 * 60)
}

fn valid_app_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

fn is_https_url(value: &str) -> bool {
    value.starts_with("https://") && value.len() > "https://".len()
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

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn version_is_newer(candidate: &str, current: &str) -> bool {
    version_parts(candidate) > version_parts(current)
}

fn version_parts(value: &str) -> Vec<u64> {
    value
        .split(['.', '-', '+'])
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn platform_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("macos", "aarch64") => "macos-aarch64",
        _ => "unsupported",
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> MullionService {
        MullionService::new(std::env::temp_dir().join(format!(
            "mullion-service-test-{}-{}",
            std::process::id(),
            unix_timestamp()
        )))
    }

    fn manifest() -> AppManifest {
        AppManifest {
            id: "net.misoworks.notes".to_string(),
            name: "Notes".to_string(),
            version: "1.0.0".to_string(),
            executable: PathBuf::from("/opt/notes/notes"),
            args: Vec::new(),
            update: Some(AppUpdateConfig {
                manifest_url: "https://updates.example.test/notes.json".to_string(),
                channel: "stable".to_string(),
                policy: UpdatePolicy::Automatic,
            }),
        }
    }

    #[test]
    fn registry_round_trips_apps() {
        let service = service();
        let registered = service.register(manifest()).unwrap();
        assert_eq!(registered.manifest.id, "net.misoworks.notes");
        assert_eq!(service.apps().unwrap().len(), 1);
        assert_eq!(service.app("net.misoworks.notes").unwrap(), registered);
        assert_eq!(
            service.unregister("net.misoworks.notes").unwrap(),
            registered
        );
    }

    #[test]
    fn registry_rejects_insecure_update_urls() {
        let service = service();
        let mut app = manifest();
        app.update.as_mut().unwrap().manifest_url = "http://example.test/app.json".to_string();
        assert!(matches!(
            service.register(app),
            Err(ServiceError::InvalidManifest(_))
        ));
    }

    #[test]
    fn update_paths_stay_inside_release_directory() {
        assert!(safe_relative_path(Path::new("bin/notes")));
        assert!(!safe_relative_path(Path::new("../notes")));
        assert!(!safe_relative_path(Path::new("/usr/bin/notes")));
    }

    #[test]
    fn update_versions_compare_numeric_segments() {
        assert!(version_is_newer("1.10.0", "1.9.9"));
        assert!(!version_is_newer("1.2.0", "1.2.0"));
        assert!(!version_is_newer("1.1.9", "1.2.0"));
    }
}
