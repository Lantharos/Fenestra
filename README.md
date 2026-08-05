![Mullion](assets/banner.png)

# Mullion

Mullion is a native application framework built around one shared Chromium runtime.
Write your UI in the web stack you already use, keep a real desktop window, and share one
browser engine across every Mullion app on the machine.

## Install

```toml
[dependencies]
mullion = { git = "https://github.com/Lantharos/Mullion" }
```

```sh
cargo install --git https://github.com/Lantharos/Mullion --package mullion-cli
cargo install --git https://github.com/Lantharos/Mullion --package mullion-service
```

For the TypeScript helpers used by the web UI:

```sh
bun add github:Lantharos/Mullion#path:packages/mullion
```

## Why Mullion

- One shared Chromium runtime across Linux, Windows, and macOS
- Native windows with GPU composition, glass materials, trays, and palettes
- Guests for embedded tabs, previews, auth flows, and untrusted pages
- Typed Rust ↔ web bridge with explicit command and origin permissions
- Automatic first-run runtime install before any page loads
- Shared service for runtime updates, app registration, and update policy
- One `Mullion.toml` for app identity, web assets, and packaging
- Lifecycle controls for background windows, tray apps, and browser-style workloads

## Quick start

```sh
mullion new my-app
cd my-app
bun install
cargo run
```

Generated apps look like this:

```rust
use mullion::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct VersionRequest {}

#[derive(Serialize)]
struct VersionResponse {
    version: &'static str,
}

fn main() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Mullion.toml");
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
```

```js
import { invoke } from "@lantharos/mullion";

const { version } = await invoke("app.version");
```

On first launch, Mullion contacts the shared service, installs the Chromium runtime if needed,
then opens your window.

## Window recipes

```rust
// Standard desktop app
MullionWindow::new().app();

// Transparent palette or launcher
MullionWindow::new().palette();

// Background tray app
MullionWindow::new()
    .tray_app()
    .tray_icon(/* ... */)
    .single_instance_id("com.example.my-app");

// Custom titlebar and sidebar glass regions
MullionWindow::new()
    .frameless()
    .glass()
    .app_chrome(AppChrome::new(38, 260));
```

Embed another page as a guest surface:

```js
const guest = await window.mullion.guest.create({
  url: "https://example.com",
  bounds: { x: 16, y: 64, width: 900, height: 600 },
  partition: "persist:browser",
  allowBridge: false,
});
```

## Configuration

`Mullion.toml` describes the app and its web assets. The CLI uses it for install and packaging;
Rust can load the same file with `with_manifest`:

```toml
[app]
id = "com.example.my-app"
name = "My App"
version = "1.0.0"
icon = "assets/icon.png"

[web]
root = "ui"
dist = "ui/dist"
entry = "ui/dist/index.html"
dev_url = "http://localhost:5173"
build = "bun run build"
allowed_origins = ["http://localhost:5173"]
```

```sh
mullion install .
mullion update
mullion bundle . --target portable --release
mullion bundle . --target deb --release
mullion bundle . --target msi --release
mullion bundle . --target dmg --release
```

## Runtime and service

Mullion keeps the Chromium runtime under the platform application-data directory. On Linux:

```text
~/.local/share/mullion/runtimes/cef/
```

`mullion-service` manages that runtime and every registered Mullion app. By default it starts at
login so the runtime is ready when you open an app. Prefer on-demand start with
`mullion-service prefer-on-demand`.

```sh
mullion runtime doctor
mullion runtime install --package standard
mullion runtime list
mullion runtime prune --keep 2

mullion-service install
mullion-service ensure
mullion-service list
mullion-service maintain
```

## Learn more

- [Implementation guide](docs/implementation-guide.md) — process model, bridge, guests, bundling, and platform notes
- [`@lantharos/mullion`](packages/mullion) — TypeScript helpers for the page bridge

## License

Mullion is dual-licensed under MIT or Apache-2.0. CEF and Chromium keep their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
