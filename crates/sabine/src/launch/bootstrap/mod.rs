mod ui;

use crate::window::config::SabineWindowConfig;
use crate::{SabineError, SabineResult};
use sabine_runtime::{RuntimeConfig, RuntimeMode, resolve_runtime};
use sabine_service::{AppManifest, SabineService, prepare_machine_with_progress};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const BOOTSTRAP_ARG: &str = "--sabine-bootstrap";

pub(crate) fn run_from_args(args: &[String]) -> bool {
    let Some(index) = args.iter().position(|arg| arg == BOOTSTRAP_ARG) else {
        return false;
    };
    let Some(path) = args.get(index + 1).map(PathBuf::from) else {
        eprintln!("missing Sabine bootstrap config");
        std::process::exit(1);
    };
    let (config, register) = match read_bootstrap(path) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let result = ui::run_progress_window("Preparing Sabine", move |state, proxy| {
        let result = prepare_machine_with_progress(config, register, |progress| {
            ui::set_progress(&state, &proxy, progress.message, progress.fraction);
        });
        ui::finish(
            &state,
            &proxy,
            result.map(|_| ()).map_err(|error| error.to_string()),
        );
    });
    if let Err(error) = result {
        eprintln!("Sabine setup failed: {error}");
        std::process::exit(1);
    }
    true
}

pub(crate) fn prepare(config: &SabineWindowConfig) -> SabineResult<()> {
    #[cfg(target_os = "linux")]
    if let Some(app_id) = config.app_id.as_deref()
        && let Err(error) = crate::desktop::integrate_appimage(app_id)
    {
        eprintln!("failed to integrate AppImage with the desktop: {error}");
    }
    let register = app_manifest(config);

    if resolve_runtime(&config.runtime).is_ok() {
        let report = sabine_service::adopt_with_runtime(config.runtime.clone(), register.clone())
            .map_err(|error| SabineError::CreationFailed {
            message: format!("failed to register with Sabine service: {error}"),
        })?;
        if std::env::var_os("SABINE_TRACE").is_some() {
            eprintln!(
                "sabine-service ready runtime={} daemon={} login_autostart={}",
                report.runtime_version, report.daemon_running, report.login_autostart
            );
        }
        relaunch_managed_update(config);
        offer_pending_update(config);
        return Ok(());
    }

    run_bootstrap_install(config, register)?;
    relaunch_managed_update(config);
    offer_pending_update(config);
    Ok(())
}

fn relaunch_managed_update(config: &SabineWindowConfig) {
    let Some(id) = config.app_id.as_deref() else {
        return;
    };
    let Ok(registered) = SabineService::default().app(id) else {
        return;
    };
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let desired = registered.manifest.executable;
    let current = current.canonicalize().unwrap_or(current);
    let desired = desired.canonicalize().unwrap_or(desired);
    if current == desired || !desired.is_file() {
        return;
    }
    let launched = Command::new(desired)
        .args(std::env::args_os().skip(1))
        .stdin(Stdio::null())
        .spawn()
        .is_ok();
    if launched {
        std::process::exit(0);
    }
}

fn offer_pending_update(config: &SabineWindowConfig) {
    let Some(id) = config.app_id.as_deref() else {
        return;
    };
    let service = SabineService::default();
    let Ok(Some(update)) = service.pending_app_update(id) else {
        return;
    };
    if !ui::confirm_update(&config.title, &update.version).unwrap_or(false) {
        return;
    }
    let Ok(service_executable) = sabine_service::resolve_service_executable() else {
        return;
    };
    let executable = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    let Some(executable) = executable else {
        return;
    };
    let spawned = Command::new(service_executable)
        .arg("apply-update")
        .arg(id)
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .arg("--relaunch")
        .arg(executable)
        .stdin(Stdio::null())
        .spawn()
        .is_ok();
    if spawned {
        std::process::exit(0);
    }
}

fn run_bootstrap_install(
    config: &SabineWindowConfig,
    register: Option<AppManifest>,
) -> SabineResult<()> {
    let config_path = bootstrap_config_path();
    write_bootstrap(&config_path, &config.runtime, register.as_ref()).map_err(|error| {
        SabineError::CreationFailed {
            message: format!("failed to prepare Sabine bootstrap: {error}"),
        }
    })?;
    let executable = std::env::current_exe().map_err(|error| SabineError::CreationFailed {
        message: format!("failed to locate app executable: {error}"),
    })?;
    let status = Command::new(executable)
        .arg(BOOTSTRAP_ARG)
        .arg(&config_path)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| SabineError::CreationFailed {
            message: format!("failed to launch Sabine bootstrap: {error}"),
        })?;
    let _ = std::fs::remove_file(config_path);
    if status.success() {
        Ok(())
    } else {
        Err(SabineError::CreationFailed {
            message: "Sabine setup did not complete".to_string(),
        })
    }
}

pub(crate) fn app_manifest(config: &SabineWindowConfig) -> Option<AppManifest> {
    let id = config.app_id.as_ref()?;
    let executable = std::env::current_exe().ok()?;
    Some(AppManifest {
        id: id.clone(),
        name: config.title.clone(),
        version: config
            .app_version
            .clone()
            .unwrap_or_else(|| "0.0.0".to_string()),
        executable,
        args: Vec::new(),
        update: config.app_update.clone(),
    })
}

fn write_bootstrap(
    path: &Path,
    config: &RuntimeConfig,
    register: Option<&AppManifest>,
) -> std::io::Result<()> {
    let mode = match config.mode {
        RuntimeMode::SystemRequired => "system-required",
        RuntimeMode::SystemPreferred => "system-preferred",
        RuntimeMode::SharedPreferred => "shared-preferred",
        RuntimeMode::Bundled => "bundled",
    };
    let mut body = serde_json::json!({
        "mode": mode,
        "index_url": config.index_url,
        "allow_user_install": config.allow_user_install,
        "allow_bundled": config.allow_bundled,
    });
    if let Some(dir) = &config.bundled_dir {
        body["bundled_dir"] = dir.display().to_string().into();
    }
    if let Some(manifest) = register {
        body["register"] = serde_json::to_value(manifest).expect("app manifest is serializable");
    }
    std::fs::write(
        path,
        serde_json::to_vec(&body).expect("bootstrap config is serializable"),
    )
}

fn read_bootstrap(path: PathBuf) -> Result<(RuntimeConfig, Option<AppManifest>), String> {
    let value = serde_json::from_slice::<Value>(&std::fs::read(&path).map_err(|e| e.to_string())?)
        .map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(path);
    let config = RuntimeConfig {
        mode: value
            .get("mode")
            .and_then(Value::as_str)
            .and_then(RuntimeMode::parse)
            .unwrap_or(RuntimeMode::SharedPreferred),
        index_url: value
            .get("index_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        allow_user_install: value
            .get("allow_user_install")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        allow_bundled: value
            .get("allow_bundled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        bundled_dir: value
            .get("bundled_dir")
            .and_then(Value::as_str)
            .map(PathBuf::from),
    };
    let register = value
        .get("register")
        .cloned()
        .map(serde_json::from_value::<AppManifest>)
        .transpose()
        .map_err(|error| format!("invalid app registration: {error}"))?;
    Ok((config, register))
}

fn bootstrap_config_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "sabine-bootstrap-{}-{nonce}.json",
        std::process::id()
    ))
}
