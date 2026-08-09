//! Notes app template.

use std::{fs, path::Path};

use super::{cargo_toml, sanitize_id};

pub(super) fn write_notes_template(root: &Path, name: &str) -> std::io::Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("ui"))?;
    fs::write(root.join("Cargo.toml"), cargo_toml(name))?;
    fs::write(root.join("Sabine.toml"), notes_sabine_toml(name))?;
    fs::write(root.join("src/main.rs"), notes_main_rs())?;
    fs::write(root.join("ui/index.html"), notes_index_html())?;
    fs::write(root.join("ui/styles.css"), notes_styles_css())?;
    fs::write(root.join("ui/app.js"), notes_app_js())?;
    Ok(())
}

fn notes_sabine_toml(name: &str) -> String {
    let id = sanitize_id(name);
    format!(
        "[app]\nid = \"dev.sabine.{id}\"\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[web]\nroot = \"ui\"\nentry = \"ui/index.html\"\n"
    )
}

fn notes_main_rs() -> &'static str {
    r#"use sabine::prelude::*;

fn main() {
    SabineWindow::main(|window| {
        Ok(window
            .size(900, 640)
            .frameless()
            .glass()
            .app_chrome(AppChrome::new(38, 260))
            .lifecycle_policy(SabineLifecyclePolicy::browser_tab())
            .bridge_handler("notes.create", |command| {
                Ok(BridgeResponse::json(serde_json::json!({
                    "ok": true,
                    "params": command.params
                })))
            }))
    });
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
  { id: "two", title: "Runtime checklist", body: "Use the shared Sabine runtime." },
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
  const bridge = window.sabine?.bridge;
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
