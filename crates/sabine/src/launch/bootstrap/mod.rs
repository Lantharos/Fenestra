mod ui;

use crate::window::SabineWindowConfig;
use crate::{SabineError, SabineResult};
use sabine_runtime::{RuntimeConfig, RuntimeMode, RuntimePackage, resolve_runtime};
use sabine_service::{AppManifest, prepare_machine_with_progress};
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
    let register = app_manifest(config);
    if resolve_runtime(&config.runtime).is_ok() {
        let report =
            sabine_service::adopt(register).map_err(|error| SabineError::CreationFailed {
                message: format!("failed to register with Sabine service: {error}"),
            })?;
        if std::env::var_os("SABINE_TRACE").is_some() {
            eprintln!(
                "sabine-service ready runtime={} daemon={} login_autostart={}",
                report.runtime_version, report.daemon_running, report.login_autostart
            );
        }
        return Ok(());
    }

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
        update: None,
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
        RuntimeMode::UserPreferred => "user-preferred",
        RuntimeMode::SharedPreferred => "shared-preferred",
        RuntimeMode::Bundled => "bundled",
        RuntimeMode::Disabled => "disabled",
    };
    let mut body = serde_json::json!({
        "mode": mode,
        "package": config.package.as_str(),
        "min_version": config.min_version,
        "index_url": config.index_url,
        "allow_user_install": config.allow_user_install,
        "allow_bundled": false,
    });
    if let Some(manifest) = register {
        body["register"] = serde_json::json!({
            "id": manifest.id,
            "name": manifest.name,
            "version": manifest.version,
            "executable": manifest.executable,
            "args": manifest.args,
        });
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
        package: value
            .get("package")
            .and_then(Value::as_str)
            .and_then(RuntimePackage::parse)
            .unwrap_or_default(),
        min_version: value
            .get("min_version")
            .and_then(Value::as_str)
            .unwrap_or("151")
            .to_string(),
        index_url: value
            .get("index_url")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        allow_user_install: value
            .get("allow_user_install")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        allow_bundled: false,
        bundled_dir: None,
        ..RuntimeConfig::default()
    };
    let register = value.get("register").and_then(|entry| {
        Some(AppManifest {
            id: entry.get("id")?.as_str()?.to_string(),
            name: entry.get("name")?.as_str()?.to_string(),
            version: entry
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("0.0.0")
                .to_string(),
            executable: PathBuf::from(entry.get("executable")?.as_str()?),
            args: entry
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            update: None,
        })
    });
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
