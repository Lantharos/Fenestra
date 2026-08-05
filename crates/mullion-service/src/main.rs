use clap::{Parser, Subcommand};
use mullion_service::{
    AppManifest, MullionService, ensure_ready, install_login_autostart_with, load_policy,
    set_login_autostart, uninstall_login_autostart,
};
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    name = "mullion-service",
    version,
    about = "Mullion runtime and app service"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install login/startup autostart for the Mullion service.
    Install,
    /// Remove login autostart, registered apps, and Mullion service data.
    Uninstall,
    /// Disable login autostart; apps will start the service on demand instead.
    PreferOnDemand,
    /// Enable login autostart again.
    PreferLogin,
    Run {
        #[arg(long, default_value_t = 21_600)]
        interval_seconds: u64,
    },
    EnsureRuntime,
    /// Ensure the daemon is running, refresh the runtime, and report status.
    Ensure,
    Maintain,
    Register {
        manifest: PathBuf,
    },
    Unregister {
        id: String,
    },
    Update {
        id: String,
    },
    List {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let service = MullionService::default();
    match cli.command {
        Command::Install => {
            set_login_autostart(true)?;
            install_login_autostart_with(&std::env::current_exe()?)?;
            println!("installed Mullion service at login");
        }
        Command::Uninstall => {
            uninstall_login_autostart()?;
            uninstall_registered_apps(&service)?;
            if service.root().is_dir() {
                std::fs::remove_dir_all(service.root())?;
            }
            println!("uninstalled Mullion and its registered apps");
        }
        Command::PreferOnDemand => {
            set_login_autostart(false)?;
            println!("Mullion will start with the first app instead of at login");
        }
        Command::PreferLogin => {
            set_login_autostart(true)?;
            install_login_autostart_with(&std::env::current_exe()?)?;
            println!("Mullion will start at login");
        }
        Command::Run { interval_seconds } => loop {
            match service.maintain() {
                Ok(report) => eprintln!(
                    "Mullion {} ready; {} app(s) registered",
                    report.runtime.version, report.registered_apps
                ),
                Err(error) => eprintln!("Mullion maintenance failed: {error}"),
            }
            std::thread::sleep(std::time::Duration::from_secs(interval_seconds.max(60)));
        },
        Command::EnsureRuntime => {
            let runtime = service.ensure_runtime_with_progress(|progress| {
                let percent = progress
                    .fraction
                    .map(|fraction| format!(" {:>3}%", (fraction * 100.0).round() as u8))
                    .unwrap_or_default();
                eprintln!("{}{}", progress.message, percent);
            })?;
            println!("{}", runtime.location.path().display());
        }
        Command::Ensure => {
            let report = ensure_ready(None)?;
            let policy = load_policy();
            println!(
                "runtime={} daemon={} login_autostart={}",
                report.runtime_version, report.daemon_running, policy.login_autostart
            );
        }
        Command::Maintain => {
            let report = service.maintain()?;
            println!(
                "runtime={} apps={} automatic_updates={} pruned_runtimes={}",
                report.runtime.version,
                report.registered_apps,
                report.automatic_updates,
                report.pruned_runtimes
            );
        }
        Command::Register { manifest } => {
            let app = serde_json::from_slice::<AppManifest>(&std::fs::read(manifest)?)?;
            println!("{}", service.register(app)?.manifest.id);
        }
        Command::Unregister { id } => {
            println!("{}", service.unregister(&id)?.manifest.id);
        }
        Command::Update { id } => {
            if service.update_app(&id)? {
                println!("updated {id}");
            } else {
                println!("{id} already up to date");
            }
        }
        Command::List { json } => {
            let apps = service.apps()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&apps)?);
            } else if apps.is_empty() {
                println!("no registered apps");
            } else {
                for app in apps {
                    println!(
                        "{}\t{}\t{}",
                        app.manifest.id, app.manifest.version, app.manifest.name
                    );
                }
            }
        }
    }
    Ok(())
}

fn uninstall_registered_apps(service: &MullionService) -> Result<(), Box<dyn std::error::Error>> {
    for app in service.apps()? {
        let id = app.manifest.id;
        let _ = service.unregister(&id);
        #[cfg(target_os = "linux")]
        if let Some(home) = std::env::var_os("HOME") {
            let home = std::path::PathBuf::from(home);
            let _ =
                std::fs::remove_file(home.join(".config/autostart").join(format!("{id}.desktop")));
        }
        #[cfg(target_os = "macos")]
        if let Some(home) = std::env::var_os("HOME") {
            let _ = std::fs::remove_file(
                std::path::PathBuf::from(home)
                    .join("Library/LaunchAgents")
                    .join(format!("{id}.plist")),
            );
        }
    }
    Ok(())
}
