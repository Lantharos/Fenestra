use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::osr_transport::IpcEndpoint;
use crate::{
    BridgeHandlers, MullionError, MullionProcess, MullionResult, MullionWindowConfig,
    browser_profile_dir, ld_library_path, prepare_bridge_command,
    process_tree::{ManagedChild, prepare_child_command},
    spawn_bridge_dispatch,
};
use mullion_bridge::{BridgeRuntime, LaunchMetrics};

pub(crate) const OSR_HOST_ARG: &str = "--mullion-osr-host";

pub(crate) fn run_from_args(args: &[String]) -> bool {
    let Some(index) = args.iter().position(|arg| arg == OSR_HOST_ARG) else {
        return false;
    };
    let Some(config_path) = args.get(index + 1).map(PathBuf::from) else {
        eprintln!("missing Mullion OSR host config path");
        std::process::exit(1);
    };
    if let Err(error) = crate::osr_host::run(config_path) {
        eprintln!("Mullion OSR host failed: {error}");
        std::process::exit(1);
    }
    true
}

pub(crate) fn launch_process(
    runtime_dir: &Path,
    config: &MullionWindowConfig,
    bridge_handlers: &BridgeHandlers,
    url: &str,
    metrics: LaunchMetrics,
) -> MullionResult<MullionProcess> {
    let host_binary = crate::ensure_host(runtime_dir)
        .map_err(|message| MullionError::CreationFailed { message })?;
    metrics.mark("host.ready");
    let host_config_path =
        std::env::temp_dir().join(format!("mullion-osr-{}.json", osr_instance_key()));
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
        "shell_surface": crate::osr_protocol::shell_surface_to_json(config.shell_surface.as_ref()),
        "background_effect": config.effective_background_effect().as_str(),
        "chrome": config.chrome.as_str(),
        "bridge_commands": mullion_bridge::bridge_commands_with_all_internal(config.bridge.commands()),
        "regions": crate::osr_protocol::regions_to_json(&config.regions),
        "drag_regions": crate::osr_protocol::rects_to_json(&config.drag_regions),
        "drag_exclusion_regions": crate::osr_protocol::rects_to_json(&config.drag_exclusion_regions),
        "control_regions": crate::osr_protocol::control_regions_to_json(&config.control_regions),
        "lifecycle": crate::osr_protocol::lifecycle_to_json(&config.lifecycle),
        "dev_mode": config.dev_mode(),
        "remote_devtools_port": config.effective_remote_devtools_port(),
        "remote_devtools_disabled": config.browser.remote_devtools_disabled,
        "hardware_decode": config.browser.hardware_decode_enabled(),
    });
    std::fs::write(&host_config_path, body.to_string()).map_err(|error| {
        MullionError::CreationFailed {
            message: format!("failed to write Mullion OSR host config: {error}"),
        }
    })?;

    let exe = std::env::current_exe().map_err(|error| MullionError::CreationFailed {
        message: error.to_string(),
    })?;
    let mut command = Command::new(exe);
    command
        .arg(OSR_HOST_ARG)
        .arg(&host_config_path)
        .stderr(Stdio::null());
    prepare_bridge_command(&mut command, bridge_handlers);
    prepare_child_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| MullionError::CreationFailed {
            message: format!("failed to launch Mullion OSR host: {error}"),
        })?;
    metrics.mark(format!("osr_host.spawned.pid.{}", child.id()));
    let activity = mullion_bridge::ActivityRegistry::default();
    let bridge_dispatch = spawn_bridge_dispatch(
        &mut child,
        BridgeRuntime::new(
            bridge_handlers.clone(),
            config.bridge.clone(),
            config.security.clone(),
        ),
        activity.clone(),
    );
    Ok(MullionProcess {
        child: ManagedChild::new(child),
        sidecars: Vec::new(),
        bridge_thread: bridge_dispatch.thread,
        bridge_emitter: bridge_dispatch.emitter,
        desktop_services: None,
        desktop_event_thread: None,
        desktop_event_running: None,
        activity,
        metrics,
    })
}

pub(crate) fn cef_osr_command(
    runtime_dir: &Path,
    host_binary: &Path,
    endpoint: &IpcEndpoint,
    authentication_token: &str,
    config: &crate::osr_host::OsrHostConfig,
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
        .unwrap_or(&config.title);
    let cache_dir = browser_profile_dir(profile_key);
    let _ = std::fs::create_dir_all(&cache_dir);
    let mut command = Command::new(host_binary);
    command
        .arg(format!("--url={}", config.url))
        .arg("--mullion-osr")
        .arg("--mullion-ozone-platform=wayland")
        .arg(format!("--mullion-osr-endpoint={}", endpoint.argument()))
        .arg(format!("--mullion-osr-token={authentication_token}"))
        .arg(format!("--mullion-width={width}"))
        .arg(format!("--mullion-height={height}"))
        .arg(format!("--mullion-scale={scale:.4}"))
        .arg(format!(
            "--mullion-bridge-commands={}",
            config.bridge_commands.join(",")
        ))
        .arg(format!(
            "--mullion-active-frame-rate={}",
            active_frame_rate.max(1)
        ))
        .arg(format!(
            "--mullion-background-frame-rate={}",
            config.lifecycle.background_frame_rate.max(1)
        ))
        .arg(format!("--root-cache-path={}", cache_dir.display()))
        .arg(format!(
            "--cache-path={}",
            cache_dir.join("browser").display()
        ));
    crate::apply_browser_launch_args(&mut command, &config.browser_options(), config.dev_mode);
    command
        .current_dir(&release_dir)
        .env("GDK_BACKEND", "wayland")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("LD_LIBRARY_PATH", ld_library_path(&release_dir));
    if config.transparent {
        command
            .arg("--mullion-transparent")
            .arg("--enable-transparent-visuals")
            .arg("--transparent-painting-enabled")
            .arg("--default-background-color=0x00000000");
    }
    command.stdin(Stdio::piped());
    if config.bridge_commands.is_empty() {
        command.stdout(Stdio::null());
    } else {
        command.stdout(Stdio::piped());
    }
    command.stderr(Stdio::null());
    command
}

fn osr_instance_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}
