use sabine_runtime::{
    RuntimeConfig, RuntimeInfo, RuntimeInstallProgress, ensure_runtime,
    install_user_runtime_with_progress, resolve_runtime,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use crate::types::{
    AppManifest, REGISTRY_VERSION, RegisteredApp, ServiceError, ServiceResult, unix_timestamp,
};

#[derive(Clone, Debug)]
pub struct SabineService {
    pub(crate) root: PathBuf,
    pub(crate) runtime: RuntimeConfig,
}

impl Default for SabineService {
    fn default() -> Self {
        Self::new(crate::types::service_data_dir())
    }
}

impl SabineService {
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

    pub(crate) fn registry_path(&self) -> PathBuf {
        self.root.join("apps.json")
    }

    pub(crate) fn load_registry(&self) -> ServiceResult<RegistryFile> {
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

    pub(crate) fn save_registry(&self, registry: &RegistryFile) -> ServiceResult<()> {
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RegistryFile {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) apps: BTreeMap<String, RegisteredApp>,
}

impl Default for RegistryFile {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            apps: BTreeMap::new(),
        }
    }
}

pub(crate) struct RegistryLock {
    path: PathBuf,
}

impl RegistryLock {
    pub(crate) fn acquire(root: &Path) -> ServiceResult<Self> {
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
