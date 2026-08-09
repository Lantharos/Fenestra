use std::{
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::process_tree::ProcessTree;
use crate::runtime;
use crate::web_detect::{self, DevProject};

pub struct DevOptions {
    pub source: PathBuf,
    pub release: bool,
    pub no_install: bool,
    pub web_only: bool,
    pub no_runtime_prepare: bool,
}

pub fn run_dev(options: DevOptions) -> Result<ExitCode, String> {
    let project = web_detect::resolve_dev_project(&options.source)?;
    if !options.no_runtime_prepare && !options.web_only {
        let _ = runtime::ensure_runtime_ready()?;
    }

    let interrupted = interrupt_flag()?;
    let mut frontend = None;
    if let Some(dev_frontend) = &project.frontend {
        if !options.no_install {
            ensure_node_modules(dev_frontend)?;
        }
        eprintln!(
            "sabine: frontend {} ({})",
            dev_frontend.url, dev_frontend.command
        );
        let mut child = spawn_frontend(dev_frontend)?;
        if !wait_for_port(
            dev_frontend.port,
            &dev_frontend.url,
            Some(&mut child),
            &interrupted,
        )? {
            return Ok(ExitCode::from(130));
        }
        frontend = Some(child);
        if options.web_only {
            eprintln!("sabine: web-only mode; Ctrl+C to stop");
            let code = wait_child(frontend.as_mut(), &interrupted)?;
            return Ok(code);
        }
    } else {
        eprintln!("sabine: static web assets (no frontend dev server)");
    }

    eprintln!(
        "sabine: cargo run --manifest-path {}",
        project.cargo_manifest.display()
    );
    let result = run_cargo(
        &project,
        options.release,
        project.frontend.as_ref(),
        frontend.as_mut(),
        &interrupted,
    );
    drop(frontend);
    result
}

fn interrupt_flag() -> Result<Arc<AtomicBool>, String> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&interrupted);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))
        .map_err(|error| format!("failed to install Ctrl+C handler: {error}"))?;
    Ok(interrupted)
}

fn ensure_node_modules(frontend: &web_detect::DevFrontend) -> Result<(), String> {
    if frontend.root.join("node_modules").is_dir() {
        return Ok(());
    }
    eprintln!(
        "sabine: installing JS deps with {} in {}",
        frontend.package_manager,
        frontend.root.display()
    );
    let status = shell_command(&format!("{} install", frontend.package_manager))
        .current_dir(&frontend.root)
        .status()
        .map_err(|error| {
            format!(
                "failed to run {} install: {error}",
                frontend.package_manager
            )
        })?;
    if !status.success() {
        return Err(format!(
            "{} install failed with status {status}",
            frontend.package_manager
        ));
    }
    Ok(())
}

fn spawn_frontend(frontend: &web_detect::DevFrontend) -> Result<ProcessTree, String> {
    let mut process = shell_command(&frontend.command);
    process
        .current_dir(&frontend.root)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    ProcessTree::spawn(&mut process)
        .map_err(|error| format!("failed to start frontend `{}`: {error}", frontend.command))
}

fn wait_for_port(
    port: u16,
    url: &str,
    mut child: Option<&mut ProcessTree>,
    interrupted: &AtomicBool,
) -> Result<bool, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_error = None;
    while Instant::now() < deadline {
        if interrupted.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if let Some(child) = child.as_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            return Err(format!(
                "frontend exited before `{url}` became available: {status}"
            ));
        }
        for host in ["127.0.0.1", "localhost"] {
            match (host, port).to_socket_addrs() {
                Ok(addresses) => {
                    for socket in addresses {
                        match TcpStream::connect_timeout(&socket, Duration::from_millis(150)) {
                            Ok(_) => return Ok(true),
                            Err(error) => last_error = Some(error),
                        }
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "timed out waiting for frontend `{url}`{}",
        last_error
            .map(|error| format!(": {error}"))
            .unwrap_or_default()
    ))
}

fn run_cargo(
    project: &DevProject,
    release: bool,
    frontend: Option<&web_detect::DevFrontend>,
    mut frontend_process: Option<&mut ProcessTree>,
    interrupted: &AtomicBool,
) -> Result<ExitCode, String> {
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--manifest-path")
        .arg(&project.cargo_manifest);
    if release {
        command.arg("--release");
    }
    if let Some(frontend) = frontend {
        command.env("SABINE_DEV_URL", &frontend.url);
    }
    if let Some(manifest) = &project.sabine_manifest {
        command.env("SABINE_MANIFEST_PATH", manifest);
    }
    let mut cargo = ProcessTree::spawn(&mut command)
        .map_err(|error| format!("failed to run cargo: {error}"))?;
    loop {
        if interrupted.load(Ordering::SeqCst) {
            let _ = cargo.terminate();
            return Ok(ExitCode::from(130));
        }
        if let Some(status) = cargo
            .try_wait()
            .map_err(|error| format!("failed waiting for cargo: {error}"))?
        {
            return Ok(exit_code(status));
        }
        if let Some(frontend) = frontend_process.as_mut()
            && let Some(status) = frontend
                .try_wait()
                .map_err(|error| format!("failed waiting for frontend: {error}"))?
        {
            let _ = cargo.terminate();
            return Err(format!(
                "frontend exited while the app was running: {status}"
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_child(
    child: Option<&mut ProcessTree>,
    interrupted: &AtomicBool,
) -> Result<ExitCode, String> {
    let Some(child) = child else {
        return Ok(ExitCode::SUCCESS);
    };
    loop {
        if interrupted.load(Ordering::SeqCst) {
            let _ = child.terminate();
            return Ok(ExitCode::from(130));
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed waiting for frontend: {error}"))?
        {
            return Ok(exit_code(status));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8)
}

fn shell_command(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut process = Command::new("cmd");
        process.args(["/C", command]);
        process
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        process
    }
}
