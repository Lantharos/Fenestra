use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    SystemRequired,
    SystemPreferred,
    UserPreferred,
    SharedPreferred,
    Bundled,
    Disabled,
}

impl RuntimeMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system-required" => Some(Self::SystemRequired),
            "system-preferred" => Some(Self::SystemPreferred),
            "user-preferred" => Some(Self::UserPreferred),
            "shared-preferred" => Some(Self::SharedPreferred),
            "bundled" => Some(Self::Bundled),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimePackage {
    Minimal,
    Client,
    #[default]
    Standard,
}

impl RuntimePackage {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "minimal" => Some(Self::Minimal),
            "client" => Some(Self::Client),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Client => "client",
            Self::Standard => "standard",
        }
    }

    pub(crate) fn install_suffix(self) -> &'static str {
        match self {
            Self::Minimal => "",
            Self::Client => "-client",
            Self::Standard => "-standard",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub version: String,
    pub location: RuntimeLocation,
    pub verified: bool,
    pub package: RuntimePackage,
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
    pub package: RuntimePackage,
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
            package: RuntimePackage::Standard,
            min_version: "144".to_string(),
            index_url: None,
            allow_user_install: true,
            allow_bundled: true,
            bundled_dir: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeInstallPlan {
    pub package: RuntimePackage,
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
