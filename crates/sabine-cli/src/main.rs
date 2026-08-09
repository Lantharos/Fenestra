mod bundle;
mod dev;
mod icon_assets;
mod process_tree;
mod runtime;
mod source_assets;
mod source_desktop;
mod source_install;
mod template;
mod web_detect;

use std::{path::PathBuf, process::ExitCode};

use bundle::BundleOptions;
use clap::{Parser, Subcommand};
use runtime::RuntimeCommand;
use source_install::{InstallOptions, UpdateOptions};

#[derive(Debug, Parser)]
#[command(name = "sabine", version, about = "Sabine web runtime tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    New {
        name: String,
        #[arg(long, default_value = "app")]
        template: String,
    },
    Dev {
        #[arg(default_value = ".")]
        source: PathBuf,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        no_install: bool,
        #[arg(long)]
        web_only: bool,
        #[arg(long)]
        no_runtime_prepare: bool,
    },
    Runtime {
        #[command(subcommand)]
        command: RuntimeSubcommand,
    },
    Install {
        #[arg(default_value = ".")]
        source: PathBuf,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        autostart: bool,
        #[arg(long)]
        no_desktop: bool,
    },
    Update {
        target: Option<String>,
        #[arg(long)]
        all: bool,
    },
    Bundle {
        #[arg(default_value = ".")]
        source: PathBuf,
        #[arg(long, default_value = "linux")]
        target: String,
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        no_build: bool,
        #[arg(long)]
        binary: Option<PathBuf>,
        #[arg(long)]
        no_web_build: bool,
        #[arg(long)]
        web_build: Option<String>,
        #[arg(long)]
        web_root: Option<PathBuf>,
        #[arg(long)]
        web_dist: Option<PathBuf>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RuntimeSubcommand {
    Prepare,
    List {
        #[arg(long)]
        json: bool,
    },
    Install,
    Remove {
        version: Option<String>,
    },
    Prune {
        #[arg(long, default_value_t = 2)]
        keep: usize,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::New { name, template } => template::new_app(&name, &template),
        Command::Dev {
            source,
            release,
            no_install,
            web_only,
            no_runtime_prepare,
        } => match dev::run_dev(dev::DevOptions {
            source,
            release,
            no_install,
            web_only,
            no_runtime_prepare,
        }) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Command::Runtime { command } => runtime::run_runtime(match command {
            RuntimeSubcommand::Prepare => RuntimeCommand::Prepare,
            RuntimeSubcommand::List { json } => RuntimeCommand::List { json },
            RuntimeSubcommand::Install => RuntimeCommand::Install,
            RuntimeSubcommand::Remove { version } => RuntimeCommand::Remove { version },
            RuntimeSubcommand::Prune { keep } => RuntimeCommand::Prune { keep },
            RuntimeSubcommand::Doctor { json } => RuntimeCommand::Doctor { json },
        }),
        Command::Install {
            source,
            id,
            name,
            command,
            autostart,
            no_desktop,
        } => match source_install::install(InstallOptions {
            source,
            id,
            name,
            command,
            autostart,
            desktop: !no_desktop,
        }) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Command::Update { target, all } => {
            match source_install::update(UpdateOptions { target, all }) {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(1)
                }
            }
        }
        Command::Bundle {
            source,
            target,
            out,
            release,
            no_build,
            binary,
            no_web_build,
            web_build,
            web_root,
            web_dist,
            id,
            name,
            version,
            json,
        } => match bundle::bundle(BundleOptions {
            source,
            target,
            out,
            release,
            no_build,
            binary,
            no_web_build,
            web_build,
            web_root,
            web_dist,
            id,
            name,
            version,
            json,
        }) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
    }
}
