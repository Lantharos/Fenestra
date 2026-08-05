mod lifecycle;
mod registry;
mod types;
mod updates;

pub use lifecycle::{
    ServicePolicy, ServiceReadyReport, ensure_daemon_running, ensure_ready,
    install_login_autostart, install_login_autostart_with, is_daemon_running, load_policy,
    policy_path, resolve_service_executable, save_policy, set_login_autostart, start_daemon,
    uninstall_login_autostart,
};
pub use registry::MullionService;
pub use types::{
    AppArtifact, AppManifest, AppReleaseManifest, AppUpdateConfig, MaintenanceReport,
    RegisteredApp, ServiceError, ServiceResult, UpdatePolicy, default_maintenance_interval,
    service_data_dir,
};

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn service() -> MullionService {
        MullionService::new(std::env::temp_dir().join(format!(
            "mullion-service-test-{}-{}",
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
        app.update.as_mut().unwrap().manifest_url = "http://example.test/app.json".to_string();
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
    fn update_versions_compare_numeric_segments() {
        assert!(types::version_is_newer("1.10.0", "1.9.9"));
        assert!(!types::version_is_newer("1.2.0", "1.2.0"));
        assert!(!types::version_is_newer("1.1.9", "1.2.0"));
    }
}
