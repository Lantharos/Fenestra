mod install;
mod lifecycle;
mod registry;
mod rollout;
mod signing;
mod types;
mod updates;

pub use install::{
    StagedSystemUpdate, cached_service_path, ensure_service_executable, find_service_executable,
    repair_system_installation, rollback_system_update, service_daemon_path, stage_system_update,
};
pub use lifecycle::{
    PrepareProgress, PrepareStage, ServicePolicy, ServiceReadyReport, adopt, adopt_with_runtime,
    complete_system_update, ensure_daemon_running, ensure_ready, ensure_ready_with_runtime,
    install_login_autostart, install_login_autostart_with, is_daemon_running, load_policy,
    policy_path, prepare_machine_with_progress, resolve_service_executable, run_daemon,
    save_policy, set_login_autostart, start_daemon, uninstall_login_autostart,
};
pub use registry::SabineService;
pub(crate) use rollout::release_is_soaked;
pub use signing::{
    public_key_from_private, sign_app_release, sign_system_release, verify_app_release,
    verify_system_release,
};
pub use types::{
    AppArtifact, AppArtifactKind, AppInstallMode, AppManifest, AppReleaseManifest, AppUpdateConfig,
    AppUpdateSource, AppUpdateStatus, MaintenanceReport, PendingAppUpdate, RegisteredApp,
    SABINE_BUILD, SABINE_MAJOR, SABINE_VERSION, SabineVersion, ServiceError, ServiceResult,
    SystemCompatibility, SystemReleaseArtifact, SystemReleaseManifest, UPDATE_ROLLOUT_WINDOW,
    UPDATE_SOAK, UpdatePolicy, default_maintenance_interval, service_data_dir, valid_app_id,
};
pub use updates::retry_quarantined_runtimes;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn service() -> SabineService {
        SabineService::new(std::env::temp_dir().join(format!(
            "sabine-service-test-{}-{}",
            std::process::id(),
            types::unix_timestamp()
        )))
    }

    fn manifest() -> AppManifest {
        AppManifest {
            id: "net.lantharos.notes".to_string(),
            name: "Notes".to_string(),
            version: "1.0.0".to_string(),
            executable: std::path::PathBuf::from("/opt/notes/notes"),
            args: Vec::new(),
            update: Some(AppUpdateConfig {
                source: AppUpdateSource::Http {
                    url: "https://updates.example.test/notes.json".to_string(),
                },
                channel: "stable".to_string(),
                policy: UpdatePolicy::Automatic,
                install_mode: AppInstallMode::Managed,
                public_key: "VXtTlN3HZuGwYByjJu+3HQGavjJwRo0i9/RGrT6Ua6M=".to_string(),
                package_kind: None,
            }),
            sabine: SabineVersion::default(),
        }
    }

    #[test]
    fn registry_round_trips_apps() {
        let service = service();
        let registered = service.register(manifest()).unwrap();
        assert_eq!(registered.manifest.id, "net.lantharos.notes");
        assert_eq!(service.apps().unwrap().len(), 1);
        assert_eq!(service.app("net.lantharos.notes").unwrap(), registered);
        assert_eq!(
            service.unregister("net.lantharos.notes").unwrap(),
            registered
        );
    }

    #[test]
    fn registry_rejects_insecure_update_urls() {
        let service = service();
        let mut app = manifest();
        app.update.as_mut().unwrap().source = AppUpdateSource::Http {
            url: "http://example.test/app.json".to_string(),
        };
        assert!(matches!(
            service.register(app),
            Err(ServiceError::InvalidManifest(_))
        ));
    }

    #[test]
    fn update_paths_stay_inside_release_directory() {
        assert!(updates::safe_relative_path(Path::new("bin/notes")));
        assert!(!updates::safe_relative_path(Path::new("../notes")));
        assert!(!updates::safe_relative_path(Path::new("/usr/bin/notes")));
    }

    #[test]
    fn update_versions_follow_semver_precedence() {
        assert!(types::version_is_newer("1.10.0", "1.9.9"));
        assert!(types::version_is_newer("1.2.0", "1.2.0-beta.1"));
        assert!(!types::version_is_newer("1.2.0-beta.1", "1.2.0"));
        assert!(!types::version_is_newer("1.2.0", "1.2.0"));
        assert!(!types::version_is_newer("1.1.9", "1.2.0"));
        assert!(!types::version_is_newer("latest", "1.2.0"));
    }

    #[test]
    fn appimage_target_and_config_names_match_their_consumers() {
        assert_eq!(AppArtifactKind::AppImage.target_suffix(), "appimage");
        assert_eq!(AppArtifactKind::AppImage.config_value(), "app-image");
    }

    #[test]
    fn runtime_quarantine_is_scoped_to_the_host_build() {
        assert!(updates::quarantine_belongs_to_host(
            "probe=current\nprobe failed",
            "probe=current"
        ));
        assert!(!updates::quarantine_belongs_to_host(
            "probe=previous\nprobe failed",
            "probe=current"
        ));
        assert!(!updates::quarantine_belongs_to_host(
            "legacy probe failure",
            "host=current"
        ));
    }
}
