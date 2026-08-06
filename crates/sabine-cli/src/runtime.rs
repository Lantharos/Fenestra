use std::process::ExitCode;

use sabine_runtime::{
    RuntimeConfig, RuntimeError, RuntimePackage, detect_runtime, latest_install_plan,
    prune_user_runtimes, remove_user_runtime_version, resolve_runtime,
    update_user_runtime_with_progress,
};

pub enum RuntimeCommand {
    Prepare,
    List {
        json: bool,
    },
    Install {
        package: String,
    },
    Remove {
        version: Option<String>,
        package: String,
    },
    Prune {
        keep: usize,
        package: String,
    },
    Doctor {
        json: bool,
    },
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
        RuntimeCommand::Install { package } => install_runtime(&package),
        RuntimeCommand::Remove { version, package } => remove_runtime(version.as_deref(), &package),
        RuntimeCommand::Prune { keep, package } => prune_runtime(keep, &package),
        RuntimeCommand::Doctor { json } => doctor_runtime(json),
    }
}

pub fn ensure_runtime_ready() -> Result<std::path::PathBuf, String> {
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
    eprintln!(
        "sabine: downloading {} runtime {}…",
        plan.package.as_str(),
        plan.version
    );
    update_user_runtime_with_progress(config, |progress| {
        let percent = progress
            .fraction
            .map(|fraction| format!(" {:>3}%", (fraction * 100.0).round() as u8))
            .unwrap_or_default();
        eprintln!("sabine: {}{}", progress.message, percent);
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
                    "{{\"package\":\"{}\",\"version\":\"{}\",\"location_type\":\"{}\",\"path\":\"{}\"}}",
                    r.package.as_str(),
                    r.version,
                    location_type,
                    r.location.path().display()
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        println!("{{\"runtimes\":[{entries}]}}");
    } else {
        if runtimes.is_empty() {
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
                    "  {} {} {} {}",
                    runtime.version,
                    runtime.package.as_str(),
                    location_type,
                    runtime.location.path().display()
                );
            }
        }
    }

    ExitCode::SUCCESS
}

fn install_runtime(package: &str) -> ExitCode {
    let Ok(config) = runtime_config(package) else {
        return ExitCode::from(1);
    };

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
            "Latest {} runtime {} is already installed at {}.",
            package,
            runtime.version,
            runtime.location.path().display()
        );
        return ExitCode::SUCCESS;
    }

    println!(
        "Installing {} runtime {}.",
        plan.package.as_str(),
        plan.version
    );
    println!("Download: {}", plan.url);
    println!("Destination: {}", plan.install_dir.display());

    match update_user_runtime_with_progress(&config, |progress| {
        let percent = progress
            .fraction
            .map(|fraction| format!(" {:>3}%", (fraction * 100.0).round() as u8))
            .unwrap_or_default();
        eprintln!("{}{}", progress.message, percent);
    }) {
        Ok(runtime) => {
            println!(
                "Installed {} runtime {} at {}.",
                runtime.package.as_str(),
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

fn remove_runtime(version: Option<&str>, package: &str) -> ExitCode {
    let Ok(config) = runtime_config(package) else {
        return ExitCode::from(1);
    };

    let Some(version) = version else {
        eprintln!("specify a version; run `sabine runtime list` to see installed versions");
        return ExitCode::from(1);
    };

    match remove_user_runtime_version(&config, version) {
        Ok(true) => {
            println!("Removed CEF {package} runtime {version}.");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            eprintln!("No user-local CEF {package} runtime {version} found.");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("failed to remove CEF {package} runtime {version}: {error}");
            ExitCode::from(1)
        }
    }
}

fn prune_runtime(keep: usize, package: &str) -> ExitCode {
    let Ok(config) = runtime_config(package) else {
        return ExitCode::from(1);
    };

    match prune_user_runtimes(&config, keep) {
        Ok(0) => {
            println!("No stale CEF {package} runtimes found.");
            ExitCode::SUCCESS
        }
        Ok(removed) => {
            println!("Removed {removed} stale CEF {package} runtime(s).");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to prune CEF {package} runtimes: {error}");
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
        .map(|runtime| sabine_host::host_release_binary(runtime.location.path()))
        .is_some_and(|path| path.is_file());
    let status = if has_compatible {
        "ok"
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
                    "{{\"version\":\"{}\",\"location\":\"{}\"}}",
                    r.version,
                    r.location.path().display()
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
                println!(
                    "{} runtime: outdated (found versions below minimum 151)",
                    "CEF"
                );
                println!("  Update with: sabine runtime install");
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

fn runtime_config(package: &str) -> Result<RuntimeConfig, ()> {
    let Some(package) = RuntimePackage::parse(package) else {
        eprintln!("unknown runtime package `{package}`; use standard, client, or minimal");
        return Err(());
    };

    Ok(RuntimeConfig {
        package,
        ..RuntimeConfig::default()
    })
}
