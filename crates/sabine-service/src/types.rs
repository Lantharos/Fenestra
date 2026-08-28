use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

pub const REGISTRY_VERSION: u32 = 1;
pub const SABINE_VERSION: &str = "0.22";
pub const SABINE_MAJOR: u32 = 0;
pub const SABINE_BUILD: u32 = 22;
pub const MIN_SUPPORTED_APP_BUILD: u32 = 1;
pub const UPDATE_SOAK: Duration = Duration::from_secs(24 * 60 * 60);
pub const UPDATE_ROLLOUT_WINDOW: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid app manifest: {0}")]
    InvalidManifest(String),
    #[error("app `{0}` is not registered")]
    AppNotFound(String),
    #[error("{message}")]
    IncompatibleApp { app_id: String, message: String },
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
    #[serde(flatten)]
    pub source: AppUpdateSource,
    #[serde(default = "stable_channel")]
    pub channel: String,
    #[serde(default)]
    pub policy: UpdatePolicy,
    #[serde(default)]
    pub install_mode: AppInstallMode,
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub package_kind: Option<AppArtifactKind>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "provider", rename_all = "kebab-case")]
pub enum AppUpdateSource {
    Github { repository: String },
    Http { url: String },
}

impl AppUpdateSource {
    pub fn manifest_url(&self, channel: &str) -> ServiceResult<String> {
        match self {
            Self::Github { repository } => {
                if channel != "stable" {
                    return Err(ServiceError::InvalidManifest(
                        "GitHub updates currently support the stable channel only".to_string(),
                    ));
                }
                if !valid_github_repository(repository) {
                    return Err(ServiceError::InvalidManifest(
                        "GitHub repository must be in owner/name form".to_string(),
                    ));
                }
                Ok(format!(
                    "https://github.com/{repository}/releases/latest/download/sabine-update.json"
                ))
            }
            Self::Http { url } if is_https_url(url) => Ok(url.clone()),
            Self::Http { .. } => Err(ServiceError::InvalidManifest(
                "update manifests must use HTTPS".to_string(),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AppInstallMode {
    #[default]
    Managed,
    Package,
    Store,
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
    #[serde(default)]
    pub sabine: SabineVersion,
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
        if let Some(update) = &self.update {
            update.source.manifest_url(&update.channel)?;
            if update.install_mode != AppInstallMode::Store
                && update.policy != UpdatePolicy::Disabled
                && update.public_key.trim().is_empty()
            {
                return Err(ServiceError::InvalidManifest(
                    "enabled updates require an Ed25519 public key".to_string(),
                ));
            }
            if update.install_mode == AppInstallMode::Package && update.package_kind.is_none() {
                return Err(ServiceError::InvalidManifest(
                    "package updates require the installed package kind".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SabineVersion {
    pub major: u32,
    pub build: u32,
}

impl SabineVersion {
    pub const fn current() -> Self {
        Self {
            major: SABINE_MAJOR,
            build: SABINE_BUILD,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let parts = value
            .trim_start_matches('v')
            .split('.')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        match parts.as_slice() {
            [major, build] => Some(Self {
                major: *major,
                build: *build,
            }),
            [major, 1, build] if *build > 0 => Some(Self {
                major: *major,
                build: *build,
            }),
            [major, build, 0] => Some(Self {
                major: *major,
                build: *build,
            }),
            _ => None,
        }
    }

    pub fn label(self) -> String {
        format!("{}.{}", self.major, self.build)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SystemCompatibility {
    pub major: u32,
    pub build: u32,
    pub minimum_app_build: u32,
}

impl Default for SystemCompatibility {
    fn default() -> Self {
        Self {
            major: SABINE_MAJOR,
            build: 0,
            minimum_app_build: 0,
        }
    }
}

impl SystemCompatibility {
    pub const fn current() -> Self {
        Self {
            major: SABINE_MAJOR,
            build: SABINE_BUILD,
            minimum_app_build: MIN_SUPPORTED_APP_BUILD,
        }
    }

    pub fn accepts(self, app: SabineVersion) -> bool {
        app.build == 0
            || (app.major == self.major
                && app.build >= self.minimum_app_build
                && app.build <= self.build)
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
    #[serde(default = "release_schema")]
    pub schema: u32,
    pub app_id: String,
    pub version: String,
    #[serde(default = "stable_channel")]
    pub channel: String,
    #[serde(default)]
    pub published_at: String,
    #[serde(default)]
    pub requires_sabine: SabineVersion,
    pub artifacts: std::collections::BTreeMap<String, AppArtifact>,
    #[serde(default)]
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SystemReleaseManifest {
    pub schema: u32,
    pub version: String,
    pub published_at: String,
    #[serde(default)]
    pub compatibility: SystemCompatibility,
    pub artifacts: std::collections::BTreeMap<String, SystemReleaseArtifact>,
    #[serde(default)]
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SystemReleaseArtifact {
    pub sha256: String,
    pub size: u64,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppArtifact {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub kind: AppArtifactKind,
    #[serde(default)]
    pub executable: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AppArtifactKind {
    #[default]
    Archive,
    Deb,
    Rpm,
    Msi,
    Exe,
    Dmg,
    AppImage,
}

impl AppArtifactKind {
    pub fn requires_elevation(self) -> bool {
        matches!(self, Self::Deb | Self::Rpm | Self::Msi)
    }

    pub fn target_suffix(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Msi => "msi",
            Self::Exe => "exe",
            Self::Dmg => "dmg",
            Self::AppImage => "appimage",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::AppImage => "app-image",
            _ => self.target_suffix(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PendingAppUpdate {
    pub app_id: String,
    pub version: String,
    pub artifact: PathBuf,
    pub sha256: String,
    pub kind: AppArtifactKind,
    pub requires_elevation: bool,
    #[serde(default)]
    pub staged_at: u64,
    #[serde(default)]
    pub prompt_after: u64,
}

impl PendingAppUpdate {
    pub fn ready_for_prompt(&self) -> bool {
        self.prompt_after <= unix_timestamp()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppUpdateStatus {
    Current,
    Deferred {
        version: String,
    },
    RequiresSystem {
        app_version: String,
        sabine: SabineVersion,
    },
    Installed {
        version: String,
    },
    PendingApproval(PendingAppUpdate),
    StoreManaged,
}

#[derive(Clone, Debug)]
pub struct MaintenanceReport {
    pub runtime: sabine_runtime::RuntimeInfo,
    pub pruned_runtimes: usize,
    pub registered_apps: usize,
    pub automatic_updates: usize,
    pub updated_apps: Vec<String>,
    pub pending_apps: Vec<String>,
    pub update_failures: Vec<String>,
    pub incompatible_apps: Vec<String>,
    pub required_system_update: Option<SabineVersion>,
}

fn release_schema() -> u32 {
    1
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

fn valid_github_repository(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !name.contains('/')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'/'))
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

pub(crate) fn update_artifact_target(
    install_mode: AppInstallMode,
    kind: Option<AppArtifactKind>,
) -> String {
    if install_mode == AppInstallMode::Package
        && let Some(kind) = kind
    {
        return format!("{}-{}", platform_target(), kind.target_suffix());
    }
    platform_target().to_string()
}

pub(crate) fn version_is_newer(candidate: &str, current: &str) -> bool {
    parse_semver(candidate)
        .ok()
        .zip(parse_semver(current).ok())
        .is_some_and(|(candidate, current)| candidate > current)
}

fn parse_semver(value: &str) -> Result<semver::Version, semver::Error> {
    let value = value.trim_start_matches('v');
    if value.bytes().filter(|byte| *byte == b'.').count() == 1 {
        semver::Version::parse(&format!("{value}.0"))
    } else {
        semver::Version::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_and_internal_versions_resolve_to_the_same_build() {
        let current = SabineVersion::current();
        assert_eq!(SabineVersion::parse(SABINE_VERSION), Some(current));
        assert_eq!(
            SabineVersion::parse(env!("CARGO_PKG_VERSION")),
            Some(current)
        );
        assert_eq!(
            SabineVersion::parse("0.1.20"),
            Some(SabineVersion {
                major: 0,
                build: 20
            })
        );
        assert!(version_is_newer("0.21", "0.1.20"));
    }

    #[test]
    fn compatibility_rejects_retired_and_future_app_builds() {
        let system = SystemCompatibility {
            major: 0,
            build: 21,
            minimum_app_build: 18,
        };
        assert!(system.accepts(SabineVersion {
            major: 0,
            build: 18
        }));
        assert!(!system.accepts(SabineVersion {
            major: 0,
            build: 17
        }));
        assert!(!system.accepts(SabineVersion {
            major: 0,
            build: 22
        }));
        assert!(!system.accepts(SabineVersion {
            major: 1,
            build: 18
        }));
    }
}
