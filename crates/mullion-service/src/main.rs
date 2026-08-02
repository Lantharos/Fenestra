use clap::{Parser, Subcommand};
use mullion_service::{AppManifest, MullionService};
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
    Install,
    Uninstall,
    Run {
        #[arg(long, default_value_t = 21_600)]
        interval_seconds: u64,
    },
    EnsureRuntime,
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
            install_service()?;
            println!("installed Mullion service");
        }
        Command::Uninstall => {
            uninstall_service()?;
            uninstall_registered_apps(&service)?;
            if service.root().is_dir() {
                std::fs::remove_dir_all(service.root())?;
            }
            println!("uninstalled Mullion and its registered apps");
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
            println!(
                "{}",
                if service.update_app(&id)? {
                    "updated"
                } else {
                    "current"
                }
            );
        }
        Command::List { json } => {
            let apps = service.apps()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&apps)?);
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
        let id = &app.manifest.id;
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("reg")
                .args([
                    "delete",
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                    "/v",
                    id,
                    "/f",
                ])
                .status();
            if let Some(app_data) = std::env::var_os("APPDATA") {
                let shortcut = std::path::Path::new(&app_data)
                    .join("Microsoft/Windows/Start Menu/Programs")
                    .join(format!("{id}.lnk"));
                let _ = std::fs::remove_file(shortcut);
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(home) = std::env::var_os("HOME") {
            let home = std::path::Path::new(&home);
            let _ = std::fs::remove_file(
                home.join(".local/share/applications")
                    .join(format!("{id}.desktop")),
            );
            let _ =
                std::fs::remove_file(home.join(".config/autostart").join(format!("{id}.desktop")));
        }
        #[cfg(target_os = "macos")]
        if let Some(home) = std::env::var_os("HOME") {
            let home = std::path::Path::new(&home);
            let app_bundle = home.join("Applications").join(format!("{id}.app"));
            if app_bundle.is_dir() {
                let _ = std::fs::remove_dir_all(app_bundle);
            }
            let _ = std::fs::remove_file(
                home.join("Library/LaunchAgents")
                    .join(format!("{id}.plist")),
            );
        }
    }
    Ok(())
}

fn install_service() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "windows")]
    {
        let command = format!("\"{}\" run", executable.display());
        run_checked(std::process::Command::new("reg").args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Mullion Service",
            "/t",
            "REG_SZ",
            "/d",
            &command,
            "/f",
        ]))?;
        let uninstall = format!("\"{}\" uninstall", executable.display());
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mullion";
        for (name, value) in [
            ("DisplayName", "Mullion".to_string()),
            ("DisplayVersion", env!("CARGO_PKG_VERSION").to_string()),
            ("Publisher", "Misoworks".to_string()),
            ("UninstallString", uninstall),
        ] {
            run_checked(
                std::process::Command::new("reg")
                    .args(["add", key, "/v", name, "/t", "REG_SZ", "/d", &value, "/f"]),
            )?;
        }
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        let directory = std::path::Path::new(&home).join(".config/systemd/user");
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("mullion.service"),
            format!(
                "[Unit]\nDescription=Mullion runtime and app service\n\n[Service]\nExecStart={} run\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
                executable.display()
            ),
        )?;
        run_checked(std::process::Command::new("systemctl").args([
            "--user",
            "enable",
            "--now",
            "mullion.service",
        ]))?;
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        let directory = std::path::Path::new(&home).join("Library/LaunchAgents");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("net.misoworks.mullion.plist");
        std::fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>net.misoworks.mullion</string><key>ProgramArguments</key><array><string>{}</string><string>run</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>\n",
                executable.display()
            ),
        )?;
        run_checked(std::process::Command::new("launchctl").args([
            "load",
            "-w",
            &path.display().to_string(),
        ]))?;
    }
    Ok(())
}

fn uninstall_service() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Mullion Service",
                "/f",
            ])
            .status();
        let _ = std::process::Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mullion",
                "/f",
            ])
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", "mullion.service"])
            .status();
        if let Some(home) = std::env::var_os("HOME") {
            let path = std::path::Path::new(&home).join(".config/systemd/user/mullion.service");
            let _ = std::fs::remove_file(path);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        let path =
            std::path::Path::new(&home).join("Library/LaunchAgents/net.misoworks.mullion.plist");
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &path.display().to_string()])
            .status();
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

fn run_checked(command: &mut std::process::Command) -> Result<(), Box<dyn std::error::Error>> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed with {status}").into())
    }
}
