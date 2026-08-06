use std::{
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::{Child, Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::runtime;
use crate::web_detect::{self, DevProject};

pub struct DevOptions {
    pub source: PathBuf,
    pub release: bool,
    pub no_install: bool,
    pub web_only: bool,
    pub no_runtime_prepare: bool,
}

struct FrontendGuard(Option<Child>);

impl FrontendGuard {
    fn take(mut self) -> Option<Child> {
        self.0.take()
    }
}

impl Drop for FrontendGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn run_dev(options: DevOptions) -> Result<ExitCode, String> {
    let project = web_detect::resolve_dev_project(&options.source)?;
    if !options.no_runtime_prepare && !options.web_only {
        let _ = runtime::ensure_runtime_ready()?;
    }

    let mut frontend = FrontendGuard(None);
    if let Some(dev_frontend) = &project.frontend {
        if !options.no_install {
            ensure_node_modules(dev_frontend)?;
        }
        eprintln!(
            "sabine: frontend {} ({})",
            dev_frontend.url, dev_frontend.command
        );
        let mut child = spawn_frontend(dev_frontend)?;
        wait_for_port(dev_frontend.port, &dev_frontend.url, Some(&mut child))?;
        frontend = FrontendGuard(Some(child));
        if options.web_only {
            eprintln!("sabine: web-only mode; Ctrl+C to stop");
            let code = wait_child(frontend.take())?;
            return Ok(code);
        }
    } else {
        eprintln!("sabine: static web assets (no frontend dev server)");
    }

    eprintln!(
        "sabine: cargo run --manifest-path {}",
        project.cargo_manifest.display()
    );
    let result = run_cargo(&project, options.release, project.frontend.as_ref());
    drop(frontend);
    result
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

fn spawn_frontend(frontend: &web_detect::DevFrontend) -> Result<Child, String> {
    let mut process = shell_command(&frontend.command);
    process
        .current_dir(&frontend.root)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    process
        .spawn()
        .map_err(|error| format!("failed to start frontend `{}`: {error}", frontend.command))
}

fn wait_for_port(port: u16, url: &str, mut child: Option<&mut Child>) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_error = None;
    while Instant::now() < deadline {
        if let Some(child) = child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "frontend exited before `{url}` became available: {status}"
                ));
            }
        }
        for host in ["127.0.0.1", "localhost"] {
            match (host, port).to_socket_addrs() {
                Ok(addresses) => {
                    for socket in addresses {
                        match TcpStream::connect_timeout(&socket, Duration::from_millis(150)) {
                            Ok(_) => return Ok(()),
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
) -> Result<ExitCode, String> {
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--manifest-path")
        .arg(&project.cargo_manifest);
    if release {
        command.arg("--release");
    }
    command.env("SABINE_SKIP_DEV_COMMAND", "1");
    if let Some(frontend) = frontend {
        command.env("SABINE_DEV_URL", &frontend.url);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to run cargo: {error}"))?;
    Ok(exit_code(status))
}

fn wait_child(child: Option<Child>) -> Result<ExitCode, String> {
    let Some(mut child) = child else {
        return Ok(ExitCode::SUCCESS);
    };
    let status = child
        .wait()
        .map_err(|error| format!("failed waiting for frontend: {error}"))?;
    Ok(exit_code(status))
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
