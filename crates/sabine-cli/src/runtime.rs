use std::process::ExitCode;

use sabine_runtime::{
    RuntimeConfig, RuntimeError, detect_runtime, latest_install_plan, prune_user_runtimes,
    remove_user_runtime_version, resolve_runtime, update_user_runtime_with_progress,
};

pub enum RuntimeCommand {
    Prepare,
    List { json: bool },
    Install,
    Remove { version: Option<String> },
    Prune { keep: usize },
    Doctor { json: bool },
}

pub fn run_runtime(command: RuntimeCommand) -> ExitCode {
    match command {
        RuntimeCommand::Prepare => match ensure_runtime_ready() {
            Ok(host) => {
                println!("{}", host.display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        RuntimeCommand::List { json } => list_runtimes(json),
        RuntimeCommand::Install => install_runtime(),
        RuntimeCommand::Remove { version } => remove_runtime(version.as_deref()),
        RuntimeCommand::Prune { keep } => prune_runtime(keep),
        RuntimeCommand::Doctor { json } => doctor_runtime(json),
    }
}

pub fn ensure_runtime_ready() -> Result<std::path::PathBuf, String> {
    sabine_service::retry_quarantined_runtimes()
        .map_err(|error| format!("failed to reconsider quarantined CEF runtimes: {error}"))?;
    let config = RuntimeConfig::default();
    let runtime = match resolve_runtime(&config) {
        Ok(runtime) => runtime,
        Err(RuntimeError::NotFound(_)) if config.allow_user_install => {
            eprintln!("sabine: no CEF runtime found; installing shared runtime…");
            install_default_runtime(&config)?
        }
        Err(error) => {
            return Err(format!(
                "failed to resolve the Sabine runtime: {error}\ninstall it with `sabine runtime install`"
            ));
        }
    };
    sabine_host::ensure_host(runtime.location.path())
        .map_err(|error| format!("failed to prepare the Sabine CEF host: {error}"))
}

fn install_default_runtime(config: &RuntimeConfig) -> Result<sabine_runtime::RuntimeInfo, String> {
    let plan = latest_install_plan(config)
        .map_err(|error| format!("failed to plan runtime install: {error}"))?;
    eprintln!("sabine: installing CEF runtime {}…", plan.version);
    let mut last_message = String::new();
    update_user_runtime_with_progress(config, |progress| {
        let percent = progress
            .fraction
            .map(|fraction| format!(" {:>3}%", (fraction * 100.0).round() as u8))
            .unwrap_or_default();
        let line = format!("{}{}", progress.message, percent);
        if line == last_message {
            return;
        }
        last_message = line.clone();
        eprintln!("sabine: {line}");
    })
    .map_err(|error| format!("failed to install runtime: {error}"))
}

fn list_runtimes(json: bool) -> ExitCode {
    let config = RuntimeConfig::default();
    let runtimes = detect_runtime(&config);
    if json {
        let entries = runtimes
            .iter()
            .map(|r| {
                let location_type = match &r.location {
                    sabine_runtime::RuntimeLocation::System(_) => "system",
                    sabine_runtime::RuntimeLocation::UserLocal(_) => "user",
                    sabine_runtime::RuntimeLocation::Bundled(_) => "bundled",
                };
                format!(
                    "{{\"version\":\"{}\",\"location_type\":\"{}\",\"path\":\"{}\",\"verified\":{}}}",
                    r.version,
                    location_type,
                    r.location.path().display(),
                    r.verified
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        println!("{{\"runtimes\":[{entries}]}}");
    } else if runtimes.is_empty() {
        println!("No CEF runtimes found.");
        println!("Run `sabine runtime install` to install the shared runtime.");
    } else {
        println!("CEF runtimes:");
        for runtime in &runtimes {
            let location_type = match &runtime.location {
                sabine_runtime::RuntimeLocation::System(_) => "system",
                sabine_runtime::RuntimeLocation::UserLocal(_) => "user",
                sabine_runtime::RuntimeLocation::Bundled(_) => "bundled",
            };
            println!(
                "  {} {} {}{}",
                runtime.version,
                location_type,
                runtime.location.path().display(),
                if runtime.verified {
                    ""
                } else {
                    " (quarantined)"
                }
            );
        }
    }

    ExitCode::SUCCESS
}

fn install_runtime() -> ExitCode {
    if let Err(error) = sabine_service::retry_quarantined_runtimes() {
        eprintln!("failed to reconsider quarantined CEF runtimes: {error}");
        return ExitCode::from(1);
    }
    let config = RuntimeConfig::default();

    let plan = match latest_install_plan(&config) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("failed to plan runtime install: {error}");
            return ExitCode::from(1);
        }
    };

    if let Ok(runtime) = resolve_runtime(&config)
        && runtime.version == plan.version
    {
        println!(
            "Latest CEF runtime {} is already installed at {}.",
            runtime.version,
            runtime.location.path().display()
        );
        return ExitCode::SUCCESS;
    }

    println!("Installing CEF runtime {}.", plan.version);
    println!("Download: {}", plan.url);
    println!("Destination: {}", plan.install_dir.display());

    match update_user_runtime_with_progress(&config, |progress| {
        let percent = progress
            .fraction
            .map(|fraction| format!(" {:>3}%", (fraction * 100.0).round() as u8))
            .unwrap_or_default();
        eprintln!("sabine: {}{}", progress.message, percent);
    }) {
        Ok(runtime) => {
            println!(
                "Installed CEF runtime {} at {}.",
                runtime.version,
                runtime.location.path().display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to install runtime: {error}");
            ExitCode::from(1)
        }
    }
}

fn remove_runtime(version: Option<&str>) -> ExitCode {
    let Some(version) = version else {
        eprintln!("specify a version; run `sabine runtime list` to see installed versions");
        return ExitCode::from(1);
    };

    match remove_user_runtime_version(version) {
        Ok(true) => {
            println!("Removed CEF runtime {version}.");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            eprintln!("No user-local CEF runtime {version} found.");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("failed to remove CEF runtime {version}: {error}");
            ExitCode::from(1)
        }
    }
}

fn prune_runtime(keep: usize) -> ExitCode {
    match prune_user_runtimes(keep) {
        Ok(0) => {
            println!("No stale CEF runtimes found.");
            ExitCode::SUCCESS
        }
        Ok(removed) => {
            println!("Removed {removed} stale CEF runtime(s).");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to prune CEF runtimes: {error}");
            ExitCode::from(1)
        }
    }
}

fn doctor_runtime(json: bool) -> ExitCode {
    let config = RuntimeConfig::default();
    let runtimes = detect_runtime(&config);
    let resolved = resolve_runtime(&config).ok();
    let has_compatible = resolved.is_some();
    let host_ready = resolved
        .as_ref()
        .and_then(|runtime| sabine_host::available_host(runtime.location.path()))
        .is_some();
    let status = if has_compatible {
        "ok"
    } else if runtimes.iter().any(|runtime| !runtime.verified) {
        "quarantined"
    } else if runtimes.is_empty() {
        "missing"
    } else {
        "outdated"
    };

    if json {
        println!(
            "{{\"status\":\"{status}\",\"host_ready\":{host_ready},\"runtimes\":[{}]}}",
            runtimes
                .iter()
                .map(|r| format!(
                    "{{\"version\":\"{}\",\"location\":\"{}\",\"verified\":{}}}",
                    r.version,
                    r.location.path().display(),
                    r.verified
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
    } else {
        match status {
            "ok" => println!("CEF runtime: ok"),
            "missing" => {
                println!("CEF runtime: not found");
                println!("  Install with: sabine runtime install");
            }
            "outdated" => {
                println!("CEF runtime: outdated (found versions below minimum 151)");
                println!("  Update with: sabine runtime install");
            }
            "quarantined" => {
                println!("CEF runtime: quarantined after a failed health probe");
                for runtime in runtimes.iter().filter(|runtime| !runtime.verified) {
                    let marker = runtime.location.path().join(".sabine-unusable");
                    if let Ok(reason) = std::fs::read_to_string(marker) {
                        println!("  {}: {}", runtime.location.path().display(), reason.trim());
                    }
                }
            }
            _ => {}
        }
        if has_compatible {
            println!(
                "Sabine host: {}",
                if host_ready {
                    "ready"
                } else {
                    "missing; run `sabine runtime prepare`"
                }
            );
        }
    }

    if has_compatible && host_ready {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
