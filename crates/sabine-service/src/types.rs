use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

pub const REGISTRY_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid app manifest: {0}")]
    InvalidManifest(String),
    #[error("app `{0}` is not registered")]
    AppNotFound(String),
    #[error("runtime operation failed: {0}")]
    Runtime(#[from] sabine_runtime::RuntimeError),
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

pub(crate) fn stable_channel() -> String {
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
    pub artifacts: std::collections::BTreeMap<String, AppArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppArtifact {
    pub url: String,
    pub sha256: String,
    pub executable: PathBuf,
}

#[derive(Clone, Debug)]
pub struct MaintenanceReport {
    pub runtime: sabine_runtime::RuntimeInfo,
    pub pruned_runtimes: usize,
    pub registered_apps: usize,
    pub automatic_updates: usize,
    pub updated_apps: Vec<String>,
    pub update_failures: Vec<String>,
}

pub fn service_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("Sabine");
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path)
            .join("Library")
            .join("Application Support")
            .join("Sabine");
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("sabine");
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join(".local/share/sabine")
}

pub fn default_maintenance_interval() -> Duration {
    Duration::from_secs(6 * 60 * 60)
}

pub fn valid_app_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

pub(crate) fn is_https_url(value: &str) -> bool {
    value.starts_with("https://") && value.len() > "https://".len()
}

pub(crate) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn platform_target() -> &'static str {
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

pub(crate) fn version_is_newer(candidate: &str, current: &str) -> bool {
    version_parts(candidate) > version_parts(current)
}

pub(crate) fn version_parts(value: &str) -> Vec<u64> {
    value
        .split(['.', '-', '+'])
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}
