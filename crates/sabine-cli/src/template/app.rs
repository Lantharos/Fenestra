//! Default Vite app template.

use std::{fs, path::Path};

use super::{cargo_toml, sanitize_id};

pub(super) fn write_app_template(root: &Path, name: &str) -> std::io::Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("ui/src"))?;
    fs::write(root.join("Cargo.toml"), cargo_toml(name))?;
    fs::write(root.join("Sabine.toml"), app_sabine_toml(name))?;
    let local_package = write_local_sabine_package(root)?;
    fs::write(
        root.join("package.json"),
        app_package_json(name, local_package),
    )?;
    fs::write(root.join("vite.config.ts"), app_vite_config())?;
    fs::write(root.join("tsconfig.json"), app_tsconfig())?;
    fs::write(root.join("src/main.rs"), app_main_rs())?;
    fs::write(root.join("ui/index.html"), app_index_html(name))?;
    fs::write(root.join("ui/src/main.ts"), app_main_ts())?;
    fs::write(root.join("ui/src/style.css"), app_style_css())?;
    Ok(())
}

fn app_sabine_toml(name: &str) -> String {
    let id = sanitize_id(name);
    format!(
        r#"[app]
id = "dev.sabine.{id}"
name = "{name}"
version = "0.1.0"
icon = "assets/icon.svg"

[web]
root = "ui"
dist = "ui/dist"
entry = "ui/dist/index.html"
dev_port = 5173
build = "bun run build"
"#
    )
}

fn app_package_json(name: &str, local_package: bool) -> String {
    let sabine_dependency = if local_package {
        "file:vendor/sabine".to_string()
    } else {
        format!("github:Lantharos/Sabine#v{}", env!("CARGO_PKG_VERSION"))
    };
    format!(
        r#"{{
  "name": "{name}",
  "private": true,
  "type": "module",
  "scripts": {{
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview"
  }},
  "dependencies": {{
    "@lantharos/sabine": "{sabine_dependency}"
  }},
  "devDependencies": {{
    "typescript": "^5.9.2",
    "vite": "^7.1.2"
  }}
}}
"#,
    )
}

fn write_local_sabine_package(root: &Path) -> std::io::Result<bool> {
    let cli_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let local = cli_dir
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("packages/sabine"));
    let Some(local) = local.filter(|path| path.join("package.json").is_file()) else {
        return Ok(false);
    };
    let destination = root.join("vendor/sabine");
    fs::create_dir_all(&destination)?;
    for name in ["package.json", "index.js", "index.d.ts", "README.md"] {
        fs::copy(local.join(name), destination.join(name))?;
    }
    Ok(true)
}

fn app_vite_config() -> &'static str {
    r#"import { defineConfig } from "vite";

export default defineConfig({
  root: "ui",
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
"#
}

fn app_tsconfig() -> &'static str {
    r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "strict": true,
    "skipLibCheck": true,
    "noEmit": true,
    "types": ["vite/client"]
  },
  "include": ["ui/src"]
}
"#
}

fn app_main_rs() -> &'static str {
    r#"use sabine::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct VersionRequest {}

#[derive(Serialize)]
struct VersionResponse {
    version: &'static str,
}

fn main() {
    SabineWindow::main(|window| {
        Ok(window
            .app()
            .size(960, 640)
            .bridge_typed("app.version", |_request: VersionRequest| {
                Ok(VersionResponse {
                    version: env!("CARGO_PKG_VERSION"),
                })
            }))
    });
}
"#
}

fn app_index_html(name: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{name}</title>
  </head>
  <body>
    <main>
      <h1>{name}</h1>
      <p id="status">Loading Sabine bridge…</p>
      <button id="version" type="button">App version</button>
    </main>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
"#
    )
}

fn app_main_ts() -> &'static str {
    r##"import { invoke } from "@lantharos/sabine";
import "./style.css";

const status = document.querySelector("#status");
const button = document.querySelector("#version");

if (status instanceof HTMLElement) {
  status.textContent = "Ready.";
}

button?.addEventListener("click", async () => {
  try {
    const result = await invoke<{ version?: string }>("app.version");
    if (status instanceof HTMLElement) {
      status.textContent = `Version ${result.version ?? "unknown"}`;
    }
  } catch (error) {
    if (status instanceof HTMLElement) {
      status.textContent = error instanceof Error ? error.message : String(error);
    }
  }
});
"##
}

fn app_style_css() -> &'static str {
    r#":root {
  color-scheme: light dark;
  font-family: ui-sans-serif, system-ui, sans-serif;
}

* {
  box-sizing: border-box;
}

html,
body {
  margin: 0;
  min-height: 100%;
}

main {
  max-width: 40rem;
  margin: 0 auto;
  padding: 3rem 1.5rem;
}

h1 {
  margin: 0 0 0.75rem;
  font-size: 2rem;
  font-weight: 600;
}

button {
  margin-top: 1rem;
  border: 0;
  border-radius: 0.5rem;
  padding: 0.65rem 1rem;
  font: inherit;
  cursor: pointer;
}
"#
}
