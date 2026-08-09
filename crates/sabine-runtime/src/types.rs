use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    SystemRequired,
    SystemPreferred,
    SharedPreferred,
    Bundled,
}

impl RuntimeMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system-required" => Some(Self::SystemRequired),
            "system-preferred" => Some(Self::SystemPreferred),
            "shared-preferred" => Some(Self::SharedPreferred),
            "bundled" => Some(Self::Bundled),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub version: String,
    pub location: RuntimeLocation,
    pub verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeLocation {
    System(PathBuf),
    UserLocal(PathBuf),
    Bundled(PathBuf),
}

impl RuntimeLocation {
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::System(p) | Self::UserLocal(p) | Self::Bundled(p) => p,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub mode: RuntimeMode,
    pub min_version: String,
    pub index_url: Option<String>,
    pub allow_user_install: bool,
    pub allow_bundled: bool,
    pub bundled_dir: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::SharedPreferred,
            min_version: "151".to_string(),
            index_url: None,
            allow_user_install: true,
            allow_bundled: true,
            bundled_dir: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeInstallPlan {
    pub version: String,
    pub platform: String,
    pub archive_name: String,
    pub url: String,
    pub sha1: String,
    pub install_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeInstallStep {
    Preparing,
    RemovingOldRuntime,
    Downloading,
    Verifying,
    Extracting,
    Installing,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeInstallProgress {
    pub step: RuntimeInstallStep,
    pub fraction: Option<f32>,
    pub message: String,
}

impl RuntimeInstallProgress {
    pub fn new(
        step: RuntimeInstallStep,
        fraction: Option<f32>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            step,
            fraction: fraction.map(|value| value.clamp(0.0, 1.0)),
            message: message.into(),
        }
    }
}
