mod detect;
mod download;
mod error;
mod host;
mod install;
mod lease;
mod paths;
mod process;
mod resolve;
mod types;
mod version;

pub(crate) const MIN_CEF_MAJOR: &str = "151";

pub use download::{DEFAULT_CEF_INDEX_URL, latest_install_plan};
pub use error::RuntimeError;
pub use install::{
    install_user_runtime, install_user_runtime_with_progress, prune_user_runtimes,
    quarantine_user_runtime, remove_user_runtime_version, update_user_runtime_with_progress,
};
pub use lease::RuntimeLease;
pub use paths::{
    bundled_runtime_path, runtime_version_path, system_runtime_path, user_runtime_path,
};
pub use process::{background_command, configure_background_command};
pub use resolve::{ensure_runtime, resolve_runtime};
pub use types::{
    RuntimeConfig, RuntimeInfo, RuntimeInstallPlan, RuntimeInstallProgress, RuntimeInstallStep,
    RuntimeLocation, RuntimeMode,
};

pub use detect::detect_runtime;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn runtime_mode_round_trips() {
        assert_eq!(
            RuntimeMode::SharedPreferred,
            RuntimeMode::parse("shared-preferred").unwrap()
        );
        assert_eq!(
            RuntimeMode::SystemRequired,
            RuntimeMode::parse("system-required").unwrap()
        );
        assert!(RuntimeMode::parse("invalid").is_none());
    }

    #[test]
    fn runtime_config_has_sane_defaults() {
        let config = RuntimeConfig::default();
        assert_eq!(config.mode, RuntimeMode::SharedPreferred);
        assert_eq!(config.index_url, None);
        assert!(config.allow_user_install);
        assert!(config.allow_bundled);
    }

    #[test]
    fn runtime_location_extracts_path() {
        let path = PathBuf::from("/usr/lib/sabine/cef");
        let loc = RuntimeLocation::System(path.clone());
        assert_eq!(loc.path(), path);
    }

    #[test]
    fn detect_runtime_skips_missing_dirs() {
        let config = RuntimeConfig::default();
        let runtimes = detect_runtime(&config);
        assert!(runtimes.is_empty() || runtimes.iter().all(|r| r.location.path().is_dir()));
    }

    #[test]
    fn version_checks_use_major_version() {
        assert!(crate::version::version_satisfies(
            "147.0.14+gabc+chromium-147.0.7727.138",
            "126"
        ));
        assert!(!crate::version::version_satisfies(
            "101.0.18+gabc+chromium-101.0.4951.67",
            "126"
        ));
    }
}
