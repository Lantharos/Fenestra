use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

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
            println!("Created Mullion app at {}", root.display());
            if template == "app" {
                println!("Install UI deps with: cd {name} && bun install");
                println!("Run it with: cargo run");
            } else {
                println!("Run it with: cargo run");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to create app: {error}");
            ExitCode::from(1)
        }
    }
}

fn write_app_template(root: &Path, name: &str) -> std::io::Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("ui/src"))?;
    fs::write(root.join("Cargo.toml"), cargo_toml(name))?;
    fs::write(root.join("Mullion.toml"), app_mullion_toml(name))?;
    fs::write(root.join("package.json"), app_package_json(name))?;
    fs::write(root.join("vite.config.ts"), app_vite_config())?;
    fs::write(root.join("tsconfig.json"), app_tsconfig())?;
    fs::write(root.join("src/main.rs"), app_main_rs())?;
    fs::write(root.join("ui/index.html"), app_index_html(name))?;
    fs::write(root.join("ui/src/main.ts"), app_main_ts())?;
    fs::write(root.join("ui/src/style.css"), app_style_css())?;
    Ok(())
}

fn write_notes_template(root: &Path, name: &str) -> std::io::Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("ui"))?;
    fs::write(root.join("Cargo.toml"), cargo_toml(name))?;
    fs::write(root.join("Mullion.toml"), notes_mullion_toml(name))?;
    fs::write(root.join("src/main.rs"), notes_main_rs())?;
    fs::write(root.join("ui/index.html"), notes_index_html())?;
    fs::write(root.join("ui/styles.css"), notes_styles_css())?;
    fs::write(root.join("ui/app.js"), notes_app_js())?;
    Ok(())
}

fn sanitize_id(name: &str) -> String {
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

fn app_mullion_toml(name: &str) -> String {
    let id = sanitize_id(name);
    format!(
        r#"[app]
id = "dev.mullion.{id}"
name = "{name}"
version = "0.1.0"

[web]
root = "ui"
dist = "ui/dist"
entry = "ui/dist/index.html"
dev_url = "http://localhost:5173"
build = "bun run build"
allowed_origins = ["http://localhost:5173"]
"#
    )
}

fn notes_mullion_toml(name: &str) -> String {
    let id = sanitize_id(name);
    format!(
        "[app]\nid = \"dev.mullion.{id}\"\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[web]\nroot = \"ui\"\nentry = \"ui/index.html\"\n"
    )
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn cargo_toml(name: &str) -> String {
    let mullion_dep = mullion_path()
        .map(|path| format!("{{ path = \"{}\" }}", path.display()))
        .unwrap_or_else(|| {
            "{ git = \"https://github.com/Lantharos/Mullion\", package = \"mullion\" }".to_string()
        });
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
mullion = {mullion_dep}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
"#
    )
}

fn mullion_path() -> Option<PathBuf> {
    let cli_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = cli_dir.parent()?.parent()?;
    let path = root.join("crates/mullion");
    path.exists().then_some(path)
}

fn app_package_json(name: &str) -> String {
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
    "@lantharos/mullion": "github:Lantharos/Mullion#path:packages/mullion"
  }},
  "devDependencies": {{
    "typescript": "^5.9.2",
    "vite": "^7.1.2"
  }}
}}
"#
    )
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
    r#"use std::path::PathBuf;

use mullion::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct VersionRequest {}

#[derive(Serialize)]
struct VersionResponse {
    version: &'static str,
}

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Mullion.toml");
    MullionWindow::main(|window| {
        Ok(window
            .with_manifest(manifest)?
            .app()
            .size(960, 640)
            .vite_dev_server(5173)
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
      <p id="status">Loading Mullion bridge…</p>
      <button id="version" type="button">App version</button>
    </main>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
"#
    )
}

fn app_main_ts() -> &'static str {
    r##"import { invoke } from "@lantharos/mullion";
import "./style.css";

const status = document.querySelector("#status");
const button = document.querySelector("#version");

if (status instanceof HTMLElement) {
  status.textContent = "Ready.";
}

button?.addEventListener("click", async () => {
  try {
    const result = (await invoke("app.version")) as { version?: string };
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

fn notes_main_rs() -> &'static str {
    r#"use std::path::PathBuf;

use mullion::{
    AppChrome, BridgeResponse, MullionWindow, run_mullion_host_from_args,
};

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if run_mullion_host_from_args(&args) {
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let window = MullionWindow::new()
        .title("Notes")
        .size(900, 640)
        .entry(format!(
            "{}?chrome=app",
            manifest_dir.join("ui/index.html").display()
        ))
        .frameless()
        .glass()
        .app_chrome(AppChrome::new(38, 260))
        .bridge_handler("notes.create", |command| {
            Ok(BridgeResponse::json(serde_json::json!({
                "ok": true,
                "params": command.params
            })))
        });

    match window.launch() {
        Ok(process) => {
            let _ = process.wait();
        }
        Err(error) => {
            eprintln!("failed to launch Mullion: {error}");
            std::process::exit(1);
        }
    }
}
"#
}

fn notes_index_html() -> &'static str {
    r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Notes</title>
    <script>
      document.documentElement.dataset.chrome =
        new URLSearchParams(location.search).get("chrome") || "system";
    </script>
    <link rel="stylesheet" href="./styles.css" />
  </head>
  <body>
    <div class="app-window">
      <div class="web-titlebar" aria-label="Window controls">
        <div class="web-titlebar-title">Notes</div>
        <div class="window-controls">
          <button class="window-control minimize" aria-label="Minimize" tabindex="-1"></button>
          <button class="window-control maximize" aria-label="Maximize" tabindex="-1"></button>
          <button class="window-control close" aria-label="Close" tabindex="-1"></button>
        </div>
      </div>
      <main class="shell">
        <aside class="sidebar">
          <h1>Notes</h1>
          <button id="new-note">New</button>
          <nav id="note-list"></nav>
          <p id="note-count">0 notes</p>
        </aside>
        <section class="content">
          <header>
            <input id="note-title" aria-label="Title" />
            <button id="save-note">Save</button>
          </header>
          <textarea id="note-body" aria-label="Body"></textarea>
        </section>
      </main>
    </div>
    <script src="./app.js" defer></script>
  </body>
</html>
"#
}

fn notes_styles_css() -> &'static str {
    r#":root {
  --window-radius: 14px;
  --titlebar-height: 38px;
  color-scheme: dark;
  font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  background: transparent;
  color: rgb(244 244 244);
}

* { box-sizing: border-box; }

html, body {
  width: 100%;
  height: 100%;
  margin: 0;
  overflow: hidden;
  background: transparent;
}

button, input, textarea {
  border: 0;
  border-radius: 8px;
  font: inherit;
  outline: none;
}

button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: 38px;
  padding: 0 18px;
  background: rgb(216 216 216);
  color: rgb(34 34 34);
  cursor: pointer;
}

.app-window {
  display: grid;
  grid-template-rows: 1fr;
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: transparent;
  border-radius: var(--window-radius);
}

:root[data-chrome="app"] .app-window {
  grid-template-rows: var(--titlebar-height) 1fr;
}

.web-titlebar {
  display: none;
  align-items: center;
  justify-content: space-between;
  padding: 0 10px 0 16px;
  -webkit-app-region: drag;
}

:root[data-chrome="app"] .web-titlebar { display: flex; }

.window-controls {
  display: flex;
  gap: 8px;
  -webkit-app-region: no-drag;
}

.window-control {
  width: 12px;
  height: 12px;
  min-height: 12px;
  padding: 0;
  border-radius: 999px;
  background: rgb(255 255 255 / 35%);
}

.shell {
  display: grid;
  grid-template-columns: 260px 1fr;
  min-height: 0;
  height: 100%;
  background: rgb(24 24 24 / 92%);
}

.sidebar, .content {
  display: grid;
  gap: 12px;
  padding: 24px;
  min-height: 0;
}

.sidebar { background: rgb(18 18 18 / 98%); }

#note-list {
  display: grid;
  gap: 8px;
  align-content: start;
  overflow: auto;
}

#note-list button {
  justify-content: flex-start;
  background: transparent;
  color: inherit;
}

#note-list button.active { background: rgb(255 255 255 / 10%); }

header {
  display: grid;
  grid-template-columns: minmax(180px, 1fr) auto;
  gap: 10px;
}

input, textarea {
  width: 100%;
  padding: 0 16px;
  background: rgb(118 118 118);
  color: rgb(248 248 248);
}

textarea {
  height: 100%;
  min-height: 0;
  padding: 16px;
  resize: none;
}
"#
}

fn notes_app_js() -> &'static str {
    r##"let notes = [
  { id: "one", title: "Product notes", body: "Keep the app monochrome, calm, and functional." },
  { id: "two", title: "Runtime checklist", body: "Use the shared Mullion runtime." },
];

let selected = 0;
const list = document.querySelector("#note-list");
const count = document.querySelector("#note-count");
const title = document.querySelector("#note-title");
const body = document.querySelector("#note-body");

function current() { return notes[selected]; }

function render() {
  list.replaceChildren(
    ...notes.map((note, index) => {
      const button = document.createElement("button");
      button.textContent = note.title;
      button.className = index === selected ? "active" : "";
      button.addEventListener("click", () => {
        saveFields();
        selected = index;
        render();
      });
      return button;
    }),
  );
  count.textContent = `${notes.length} notes`;
  title.value = current().title;
  body.value = current().body;
}

function saveFields() {
  current().title = title.value || "Untitled";
  current().body = body.value;
}

document.querySelector("#new-note").addEventListener("click", async () => {
  saveFields();
  const bridge = window.mullion?.bridge;
  const created = bridge?.commands?.includes("notes.create")
    ? await bridge.invoke("notes.create", { title: "Untitled" })
    : null;
  notes.push({ id: created?.id || crypto.randomUUID(), title: "Untitled", body: "" });
  selected = notes.length - 1;
  render();
  title.focus();
});

document.querySelector("#save-note").addEventListener("click", () => {
  saveFields();
  render();
});

render();
"##
}
