//! `sabine new` project templates.

mod app;
mod notes;

use std::{path::PathBuf, process::ExitCode};

use app::write_app_template;
use notes::write_notes_template;

pub fn new_app(name: &str, template: &str) -> ExitCode {
    if !matches!(template, "app" | "notes") {
        eprintln!("unknown template `{template}`; available templates: app, notes");
        return ExitCode::from(1);
    }
    if !is_valid_name(name) {
        eprintln!("app name must contain only letters, numbers, hyphens, and underscores");
        return ExitCode::from(1);
    }

    let root = PathBuf::from(name);
    if root.exists() {
        eprintln!("{} already exists", root.display());
        return ExitCode::from(1);
    }

    let result = match template {
        "notes" => write_notes_template(&root, name),
        _ => write_app_template(&root, name),
    };
    match result {
        Ok(()) => {
            println!("Created Sabine app at {}", root.display());
            println!("Run it with: cd {name} && sabine dev");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to create app: {error}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn sanitize_id(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(crate) fn cargo_toml(name: &str) -> String {
    let sabine_dep = sabine_path()
        .map(|path| format!("{{ path = \"{}\" }}", path.display()))
        .unwrap_or_else(|| {
            "{ git = \"https://github.com/Lantharos/Sabine\", package = \"sabine\" }".to_string()
        });
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
sabine = {sabine_dep}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
"#
    )
}

fn sabine_path() -> Option<PathBuf> {
    let cli_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = cli_dir.parent()?.parent()?;
    let path = root.join("crates/sabine");
    path.exists().then_some(path)
}
