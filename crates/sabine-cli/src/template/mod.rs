//! `sabine new` project templates.

mod app;
mod notes;

use std::{fs, path::PathBuf, process::ExitCode};

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
    }
    .and_then(|_| write_default_icon(&root))
    .and_then(|_| write_release_workflow(&root));
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

fn write_default_icon(root: &std::path::Path) -> std::io::Result<()> {
    let assets = root.join("assets");
    fs::create_dir_all(&assets)?;
    fs::write(
        assets.join("icon.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">
  <rect width="512" height="512" rx="112" fill="#17191f"/>
  <path d="M354 153c-25-24-59-37-99-37-65 0-111 34-111 84 0 54 47 70 104 82 47 10 69 18 69 42 0 25-24 39-61 39-43 0-80-16-108-47l-35 38c35 39 84 59 140 59 69 0 116-35 116-88 0-55-43-73-106-87-43-9-67-17-67-39 0-22 22-35 58-35 34 0 64 11 88 33z" fill="#f4f5f7"/>
</svg>
"##,
    )
}

fn write_release_workflow(root: &std::path::Path) -> std::io::Result<()> {
    let workflows = root.join(".github/workflows");
    fs::create_dir_all(&workflows)?;
    fs::write(
        workflows.join("release.yml"),
        format!(
            r#"name: Release

on:
  push:
    tags:
      - "v*"

permissions:
  contents: write
  id-token: write
  attestations: write

jobs:
  release:
    uses: Lantharos/Sabine/.github/workflows/release-app.yml@v{version}
    with:
      sabine_version: v{version}
    secrets:
      SABINE_UPDATE_SIGNING_KEY: ${{{{ secrets.SABINE_UPDATE_SIGNING_KEY }}}}
"#,
            version = sabine_service::SABINE_VERSION
        ),
    )
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
        .map(|path| {
            let path = path.display().to_string().replace('\\', "\\\\");
            format!("{{ path = \"{path}\" }}")
        })
        .unwrap_or_else(|| {
            format!(
                "{{ git = \"https://github.com/Lantharos/Sabine\", tag = \"v{}\", package = \"sabine\" }}",
                sabine_service::SABINE_VERSION
            )
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

[workspace]
"#
    )
}

fn sabine_path() -> Option<PathBuf> {
    let cli_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = cli_dir.parent()?.parent()?;
    let path = root.join("crates/sabine");
    path.exists().then_some(path)
}
