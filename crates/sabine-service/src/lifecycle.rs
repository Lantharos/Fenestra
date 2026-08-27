use sabine_runtime::RuntimeConfig;

use crate::{AppManifest, SabineService, ServiceError, ServiceResult, service_data_dir};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

mod autostart;
mod daemon;

pub use autostart::{
    install_login_autostart, install_login_autostart_with, uninstall_login_autostart,
};
pub use daemon::{
    complete_system_update, ensure_daemon_running, is_daemon_running, resolve_service_executable,
    run_daemon, start_daemon,
};

const POLICY_FILE: &str = "service-policy.json";
pub(super) const PID_FILE: &str = "service.pid";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServicePolicy {
    pub login_autostart: bool,
}

impl Default for ServicePolicy {
    fn default() -> Self {
        Self {
            login_autostart: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceReadyReport {
    pub login_autostart: bool,
    pub daemon_running: bool,
    pub runtime_version: String,
    pub registered_app: Option<String>,
}

pub fn policy_path() -> PathBuf {
    service_data_dir().join(POLICY_FILE)
}

pub fn load_policy() -> ServicePolicy {
    let path = policy_path();
    let Ok(bytes) = fs::read(&path) else {
        return ServicePolicy::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save_policy(policy: &ServicePolicy) -> ServiceResult<()> {
    fs::create_dir_all(service_data_dir())?;
    let bytes = serde_json::to_vec_pretty(policy)
        .map_err(|error| ServiceError::Update(error.to_string()))?;
    fs::write(policy_path(), bytes)?;
    Ok(())
}

pub fn set_login_autostart(enabled: bool) -> ServiceResult<ServicePolicy> {
    let mut policy = load_policy();
    policy.login_autostart = enabled;
    save_policy(&policy)?;
    if enabled {
        install_login_autostart()?;
    } else {
        uninstall_login_autostart()?;
    }
    Ok(policy)
}

pub fn ensure_ready(register: Option<AppManifest>) -> ServiceResult<ServiceReadyReport> {
    ensure_ready_with_runtime(RuntimeConfig::default(), register)
}

pub fn ensure_ready_with_runtime(
    runtime: sabine_runtime::RuntimeConfig,
    register: Option<AppManifest>,
) -> ServiceResult<ServiceReadyReport> {
    let policy = load_policy();
    let _ = save_policy(&policy);

    if policy.login_autostart {
        let _ = install_login_autostart();
    }

    let daemon_running = ensure_daemon_running().unwrap_or(false);
    let service = SabineService::default().with_runtime(runtime);
    let report = service.maintain()?;

    let registered_app = if let Some(manifest) = register {
        Some(service.register(manifest)?.manifest.id)
    } else {
        None
    };

    Ok(ServiceReadyReport {
        login_autostart: policy.login_autostart,
        daemon_running,
        runtime_version: report.runtime.version,
        registered_app,
    })
}

pub fn adopt(register: Option<AppManifest>) -> ServiceResult<ServiceReadyReport> {
    adopt_with_runtime(sabine_runtime::RuntimeConfig::default(), register)
}

pub fn adopt_with_runtime(
    runtime: sabine_runtime::RuntimeConfig,
    register: Option<AppManifest>,
) -> ServiceResult<ServiceReadyReport> {
    let policy = load_policy();
    let daemon_running = ensure_daemon_running().unwrap_or(false);
    let service = SabineService::default().with_runtime(runtime);
    let runtime = service.runtime()?;
    let registered_app = if let Some(manifest) = register {
        Some(service.register(manifest)?.manifest.id)
    } else {
        None
    };

    Ok(ServiceReadyReport {
        login_autostart: policy.login_autostart,
        daemon_running,
        runtime_version: runtime.version,
        registered_app,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareStage {
    Service,
    Runtime,
    Register,
}

#[derive(Clone, Debug)]
pub struct PrepareProgress {
    pub stage: PrepareStage,
    pub message: String,
    pub fraction: Option<f32>,
}

pub fn prepare_machine_with_progress(
    runtime: sabine_runtime::RuntimeConfig,
    register: Option<AppManifest>,
    mut on_progress: impl FnMut(PrepareProgress),
) -> ServiceResult<ServiceReadyReport> {
    on_progress(PrepareProgress {
        stage: PrepareStage::Service,
        message: "Starting Sabine service".to_string(),
        fraction: Some(0.05),
    });

    let _ = crate::ensure_service_executable(&mut on_progress)?;

    let policy = load_policy();
    let _ = save_policy(&policy);

    on_progress(PrepareProgress {
        stage: PrepareStage::Runtime,
        message: "Preparing runtime".to_string(),
        fraction: Some(0.1),
    });

    let service = SabineService::default().with_runtime(runtime);
    let runtime_info = service.ensure_runtime_with_progress(|progress| {
        let fraction = progress
            .fraction
            .map(|value| 0.1 + value.clamp(0.0, 1.0) * 0.8)
            .or(Some(0.1));
        on_progress(PrepareProgress {
            stage: PrepareStage::Runtime,
            message: progress.message,
            fraction,
        });
    })?;

    if policy.login_autostart {
        let _ = install_login_autostart();
    }
    let daemon_running = ensure_daemon_running().unwrap_or(false);

    let registered_app = if let Some(manifest) = register {
        on_progress(PrepareProgress {
            stage: PrepareStage::Register,
            message: format!("Registering {}", manifest.name),
            fraction: Some(0.95),
        });
        Some(service.register(manifest)?.manifest.id)
    } else {
        None
    };

    on_progress(PrepareProgress {
        stage: PrepareStage::Register,
        message: "Sabine is ready".to_string(),
        fraction: Some(1.0),
    });

    Ok(ServiceReadyReport {
        login_autostart: policy.login_autostart,
        daemon_running,
        runtime_version: runtime_info.version,
        registered_app,
    })
}
