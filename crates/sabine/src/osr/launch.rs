use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::host::{ManagedChild, prepare_child_command};
use crate::osr::transport::IpcEndpoint;
use crate::{
    BridgeHandlers, SabineError, SabineProcess, SabineResult, SabineWindowConfig,
    browser_profile_dir, ld_library_path, prepare_bridge_command, spawn_bridge_dispatch,
    spawn_bridge_dispatch_for_window,
};
use sabine_bridge::{BridgeRuntime, LaunchMetrics};

pub(crate) const OSR_HOST_ARG: &str = "--sabine-osr-host";

pub(crate) fn run_from_args(args: &[String]) -> bool {
    let Some(index) = args.iter().position(|arg| arg == OSR_HOST_ARG) else {
        return false;
    };
    let Some(config_path) = args.get(index + 1).map(PathBuf::from) else {
        eprintln!("missing Sabine OSR host config path");
        std::process::exit(1);
    };
    if let Err(error) = crate::osr::host::run(config_path) {
        eprintln!("Sabine OSR host failed: {error}");
        std::process::exit(1);
    }
    true
}

pub(crate) fn require_app_id(config: &SabineWindowConfig) -> SabineResult<&str> {
    config
        .app_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SabineError::CreationFailed {
            message: "Sabine requires a non-empty app_id (set via .app_id(...) or with_manifest)"
                .into(),
        })
}

pub(crate) fn launch_process(
    runtime_dir: &Path,
    config: &SabineWindowConfig,
    bridge_handlers: &BridgeHandlers,
    url: &str,
    metrics: LaunchMetrics,
) -> SabineResult<SabineProcess> {
    let app_id = require_app_id(config)?.to_string();
    let host_binary = crate::ensure_host(runtime_dir)
        .map_err(|message| SabineError::CreationFailed { message })?;
    metrics.mark("host.ready");
    let mut child = spawn_osr_host_child(runtime_dir, &host_binary, config, url)?;
    metrics.mark(format!("osr_host.spawned.pid.{}", child.id()));
    let activity = sabine_bridge::ActivityRegistry::default();
    let bridge_dispatch = spawn_bridge_dispatch(
        &mut child,
        BridgeRuntime::new(
            bridge_handlers.clone(),
            config.bridge.clone(),
            config.security.clone(),
        ),
        activity.clone(),
    );
    Ok(SabineProcess {
        child: ManagedChild::new(child),
        primary_alive: true,
        primary_status: None,
        sidecars: Vec::new(),
        extra_windows: Vec::new(),
        bridge_thread: bridge_dispatch.thread,
        extra_bridge_threads: Vec::new(),
        bridge_emitter: bridge_dispatch.emitter,
        desktop_services: None,
        desktop_event_thread: None,
        desktop_event_running: None,
        activity,
        metrics,
        open_window: Some(OpenWindowContext {
            runtime_dir: runtime_dir.to_path_buf(),
            host_binary,
            app_id,
            bridge_handlers: bridge_handlers.clone(),
            bridge: config.bridge.clone(),
            security: config.security.clone(),
        }),
    })
}

pub(crate) struct OpenWindowContext {
    pub(crate) runtime_dir: PathBuf,
    pub(crate) host_binary: PathBuf,
    pub(crate) app_id: String,
    pub(crate) bridge_handlers: BridgeHandlers,
    pub(crate) bridge: sabine_bridge::BridgeRegistry,
    pub(crate) security: sabine_bridge::ContentSecurity,
}

pub(crate) fn spawn_osr_host_child(
    runtime_dir: &Path,
    host_binary: &Path,
    config: &SabineWindowConfig,
    url: &str,
) -> SabineResult<std::process::Child> {
    let _ = require_app_id(config)?;
    let host_config_path =
        std::env::temp_dir().join(format!("sabine-osr-{}.json", osr_instance_key()));
    let body = serde_json::json!({
        "runtime_dir": runtime_dir,
        "host_binary": host_binary,
        "url": url,
        "app_id": config.app_id,
        "title": config.title,
        "width": config.width,
        "height": config.height,
        "min_width": config.min_width,
        "min_height": config.min_height,
        "resizable": config.resizable,
        "visible": config.visible,
        "shell_surface_alpha": config.shell_surface_alpha,
        "active": config.active,
        "hide_on_blur": config.hide_on_blur,
        "always_on_top": config.always_on_top,
        "transparent": config.transparent,
        "shell_surface": crate::osr::protocol::shell_surface_to_json(config.shell_surface.as_ref()),
        "background_effect": config.effective_background_effect().as_str(),
        "chrome": config.chrome.as_str(),
        "bridge_commands": sabine_bridge::bridge_commands_with_all_internal(config.bridge.commands()),
        "regions": crate::osr::protocol::regions_to_json(&config.regions),
        "drag_regions": crate::osr::protocol::rects_to_json(&config.drag_regions),
        "drag_exclusion_regions": crate::osr::protocol::rects_to_json(&config.drag_exclusion_regions),
        "control_regions": crate::osr::protocol::control_regions_to_json(&config.control_regions),
        "lifecycle": crate::osr::protocol::lifecycle_to_json(&config.lifecycle),
        "dev_mode": config.dev_mode(),
        "remote_devtools_port": config.effective_remote_devtools_port(),
        "remote_devtools_disabled": config.browser.remote_devtools_disabled,
        "hardware_decode": config.browser.hardware_decode_enabled(),
    });
    std::fs::write(&host_config_path, body.to_string()).map_err(|error| {
        SabineError::CreationFailed {
            message: format!("failed to write Sabine OSR host config: {error}"),
        }
    })?;

    let exe = std::env::current_exe().map_err(|error| SabineError::CreationFailed {
        message: error.to_string(),
    })?;
    let mut command = Command::new(exe);
    command
        .arg(OSR_HOST_ARG)
        .arg(&host_config_path)
        .stderr(Stdio::inherit());
    prepare_bridge_command(&mut command, &BridgeHandlers::default());
    prepare_child_command(&mut command);
    command
        .spawn()
        .map_err(|error| SabineError::CreationFailed {
            message: format!("failed to launch Sabine OSR host: {error}"),
        })
}

pub(crate) fn attach_open_window(
    process: &mut SabineProcess,
    config: &SabineWindowConfig,
    url: &str,
) -> SabineResult<u32> {
    let context = process
        .open_window
        .as_ref()
        .ok_or_else(|| SabineError::CreationFailed {
            message: "this Sabine process does not support open_window".into(),
        })?;
    let mut window_config = config.clone();
    window_config.app_id = Some(context.app_id.clone());
    let mut child = spawn_osr_host_child(
        &context.runtime_dir,
        &context.host_binary,
        &window_config,
        url,
    )?;
    let window_id = child.id();
    let Some(emitter) = process.bridge_emitter.clone() else {
        return Err(SabineError::CreationFailed {
            message: "bridge emitter is unavailable for open_window".into(),
        });
    };
    let thread = spawn_bridge_dispatch_for_window(
        &mut child,
        BridgeRuntime::new(
            context.bridge_handlers.clone(),
            context.bridge.clone(),
            context.security.clone(),
        ),
        process.activity.clone(),
        &emitter,
    );
    if let Some(thread) = thread {
        process.extra_bridge_threads.push(thread);
    }
    process.extra_windows.push(ManagedChild::new(child));
    Ok(window_id)
}

pub(crate) fn cef_osr_command(
    runtime_dir: &Path,
    host_binary: &Path,
    endpoint: &IpcEndpoint,
    authentication_token: &str,
    config: &crate::osr::host::OsrHostConfig,
    width: u32,
    height: u32,
    scale: f64,
    active_frame_rate: u32,
) -> Command {
    let release_dir = runtime_dir.join("Release");
    let profile_key = config
        .app_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .expect("cef_osr_command requires a non-empty app_id");
    let cache_dir = browser_profile_dir(profile_key);
    let _ = std::fs::create_dir_all(&cache_dir);
    let token_file = crate::osr::transport::write_token_file(endpoint, authentication_token)
        .unwrap_or_else(|error| {
            eprintln!("failed to write OSR token file: {error}");
            PathBuf::new()
        });
    let mut command = Command::new(host_binary);
    command
        .arg(format!("--url={}", config.url))
        .arg("--sabine-osr")
        .arg("--sabine-ozone-platform=wayland")
        .arg(format!("--sabine-osr-endpoint={}", endpoint.argument()));
    if !token_file.as_os_str().is_empty() {
        command.arg(format!("--sabine-osr-token-file={}", token_file.display()));
    }
    command
        .arg(format!("--sabine-width={width}"))
        .arg(format!("--sabine-height={height}"))
        .arg(format!("--sabine-scale={scale:.4}"))
        .arg(format!(
            "--sabine-bridge-commands={}",
            config.bridge_commands.join(",")
        ))
        .arg(format!(
            "--sabine-active-frame-rate={}",
            active_frame_rate.max(1)
        ))
        .arg(format!(
            "--sabine-background-frame-rate={}",
            config.lifecycle.background_frame_rate.max(1)
        ))
        .arg(format!("--root-cache-path={}", cache_dir.display()))
        .arg(format!(
            "--cache-path={}",
            cache_dir.join("browser").display()
        ));
    crate::apply_browser_launch_args(&mut command, &config.browser_options(), config.dev_mode);
    crate::host::prepare_detachable_child_command(&mut command);
    command
        .current_dir(&release_dir)
        .env("GDK_BACKEND", "wayland")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("LD_LIBRARY_PATH", ld_library_path(&release_dir));
    // Env remains a fallback for non-handoff launches; the token file is what
    // survives CEF process-singleton relaunch into the primary process.
    command.env(crate::osr::transport::OSR_TOKEN_ENV, authentication_token);
    if config.transparent {
        command
            .arg("--sabine-transparent")
            .arg("--enable-transparent-visuals")
            .arg("--transparent-painting-enabled")
            .arg("--default-background-color=0x00000000");
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::inherit());
    command
}

fn osr_instance_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}
