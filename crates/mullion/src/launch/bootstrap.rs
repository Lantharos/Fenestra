use crate::window::MullionWindowConfig;
use crate::{MullionError, MullionResult};
use mullion_runtime::{RuntimeConfig, RuntimeMode, RuntimePackage, resolve_runtime};
use mullion_service::{AppManifest, PrepareProgress, prepare_machine_with_progress};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

pub(crate) const BOOTSTRAP_ARG: &str = "--mullion-bootstrap";

pub(crate) fn run_from_args(args: &[String]) -> bool {
    let Some(index) = args.iter().position(|arg| arg == BOOTSTRAP_ARG) else {
        return false;
    };
    let Some(path) = args.get(index + 1).map(PathBuf::from) else {
        eprintln!("missing Mullion bootstrap config");
        std::process::exit(1);
    };
    let (config, register) = match read_bootstrap(path) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let event_loop = EventLoop::new().unwrap_or_else(|error| {
        eprintln!("failed to create Mullion bootstrap window: {error}");
        std::process::exit(1);
    });
    let proxy = event_loop.create_proxy();
    let state = Arc::new(Mutex::new(BootstrapState {
        message: "Preparing Mullion".to_string(),
        ..BootstrapState::default()
    }));
    let worker_state = Arc::clone(&state);
    let worker_proxy = proxy.clone();
    thread::spawn(move || {
        let result = prepare_machine_with_progress(config, register, |progress| {
            update_progress(&worker_state, &progress);
            worker_proxy.wake_up();
        });
        if let Ok(mut state) = worker_state.lock() {
            state.done = Some(result.map(|report| report.runtime_version));
        }
        worker_proxy.wake_up();
    });
    let app = BootstrapApp {
        state: Arc::clone(&state),
        window: None,
    };
    if let Err(error) = event_loop.run_app(app) {
        eprintln!("Mullion bootstrap failed: {error}");
        std::process::exit(1);
    }
    let succeeded = state
        .lock()
        .ok()
        .and_then(|state| state.done.as_ref().map(Result::is_ok))
        .unwrap_or(false);
    if !succeeded {
        std::process::exit(1);
    }
    true
}

pub(crate) fn prepare(config: &MullionWindowConfig) -> MullionResult<()> {
    let register = app_manifest(config);
    if resolve_runtime(&config.runtime).is_ok() {
        match mullion_service::adopt(register) {
            Ok(report) => {
                if std::env::var_os("MULLION_TRACE").is_some() {
                    eprintln!(
                        "mullion-service ready runtime={} daemon={} login_autostart={}",
                        report.runtime_version, report.daemon_running, report.login_autostart
                    );
                }
            }
            Err(error) => eprintln!("mullion-service: {error}"),
        }
        return Ok(());
    }

    let config_path = bootstrap_config_path();
    write_bootstrap(&config_path, &config.runtime, register.as_ref()).map_err(|error| {
        MullionError::CreationFailed {
            message: format!("failed to prepare Mullion bootstrap: {error}"),
        }
    })?;
    let executable = std::env::current_exe().map_err(|error| MullionError::CreationFailed {
        message: format!("failed to locate app executable: {error}"),
    })?;
    let status = Command::new(executable)
        .arg(BOOTSTRAP_ARG)
        .arg(&config_path)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| MullionError::CreationFailed {
            message: format!("failed to launch Mullion bootstrap: {error}"),
        })?;
    let _ = std::fs::remove_file(config_path);
    if status.success() {
        Ok(())
    } else {
        Err(MullionError::CreationFailed {
            message: "Mullion setup did not complete".to_string(),
        })
    }
}

pub(crate) fn app_manifest(config: &MullionWindowConfig) -> Option<AppManifest> {
    let id = config.app_id.as_ref()?;
    let executable = std::env::current_exe().ok()?;
    Some(AppManifest {
        id: id.clone(),
        name: config.title.clone(),
        version: "0.0.0".to_string(),
        executable,
        args: Vec::new(),
        update: None,
    })
}

#[derive(Default)]
struct BootstrapState {
    message: String,
    fraction: Option<f32>,
    done: Option<Result<String, mullion_service::ServiceError>>,
}

struct BootstrapApp {
    state: Arc<Mutex<BootstrapState>>,
    window: Option<Box<dyn Window>>,
}

impl ApplicationHandler for BootstrapApp {
    fn resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title("Preparing Mullion")
            .with_surface_size(LogicalSize::new(460.0, 150.0))
            .with_resizable(false)
            .with_decorations(true);
        match event_loop.create_window(attributes) {
            Ok(window) => {
                self.window = Some(window);
                self.refresh(event_loop);
            }
            Err(error) => {
                eprintln!("failed to open Mullion bootstrap window: {error}");
                event_loop.exit();
            }
        }
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.resumed(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested) {
            event_loop.exit();
        }
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.refresh(event_loop);
    }
}

impl BootstrapApp {
    fn refresh(&mut self, event_loop: &dyn ActiveEventLoop) {
        let Ok(state) = self.state.lock() else {
            event_loop.exit();
            return;
        };
        if let Some(done) = &state.done {
            if let Err(error) = done {
                eprintln!("Mullion setup failed: {error}");
            }
            event_loop.exit();
            return;
        }
        if let Some(window) = &self.window {
            let percent = state
                .fraction
                .map(|fraction| format!(" — {}%", (fraction * 100.0).round() as u8))
                .unwrap_or_default();
            let message = if state.message.is_empty() {
                "Preparing Mullion"
            } else {
                &state.message
            };
            window.set_title(&format!("{message}{percent}"));
        }
    }
}

fn update_progress(state: &Mutex<BootstrapState>, progress: &PrepareProgress) {
    if let Ok(mut state) = state.lock() {
        state.message = progress.message.clone();
        state.fraction = progress.fraction;
    }
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
            .unwrap_or("144")
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
        "mullion-bootstrap-{}-{nonce}.json",
        std::process::id()
    ))
}
