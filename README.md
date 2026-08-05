![Mullion](assets/banner.png)

# Mullion

Mullion is a native application framework built around one shared Chromium runtime. Every desktop
uses the same off-screen-rendering pipeline, the same bridge, the same guest model, and the same
runtime service. Apps keep native windows, GPU composition, tray and palette behavior, background
work, and platform integration without shipping a browser engine per app.

The project is early and intentionally fast-moving; pre-1.0 APIs may change when that improves the design.

## Install from Git

Mullion is distributed from GitHub, not crates.io. Add it to your app with a git dependency:

```toml
[dependencies]
mullion = { git = "https://github.com/Lantharos/Mullion" }
```

Install the CLI and shared service the same way:

```sh
cargo install --git https://github.com/Lantharos/Mullion --package mullion-cli
cargo install --git https://github.com/Lantharos/Mullion --package mullion-service
```

## Why Mullion

- One shared Chromium runtime across Linux, Windows, and macOS
- OSR-only rendering on every desktop
- Damage-driven GPU composition with dirty-rect uploads
- Embedded guests for browser tabs, previews, authentication, and untrusted pages
- Typed Rust-to-web bridge with explicit command and origin permissions
- Native first-run runtime installer before any web content starts
- Shared service that oversees registered apps, runtime updates, pruning, and update policy
- Windows, macOS, and Linux bundle staging from one `Mullion.toml`
- Tray apps, hidden windows, palettes, global shortcuts, deep links, autostart, and single-instance apps
- Lifecycle throttling and hibernation for background windows and browser-style workloads

## Architecture

```text
app process
  MullionWindow builder + native services
        |
        +-- native OSR window (winit + wgpu)
        |      |
        |      +-- Mullion host process
        |      +-- main page and guest surfaces
        |      +-- damage-only texture uploads
        |
        +-- mullion-service
               +-- shared Chromium runtime
               +-- registered app catalog
               +-- runtime updates and pruning
               +-- app update channels and policy
```

Linux uses a Unix socket and shared-memory paint batches for large updates. macOS uses the same local
Unix transport and falls back to inline dirty rectangles when shared buffers are unavailable. Windows
uses authenticated loopback transport and inline dirty rectangles. None of these paths
switches to full-frame redraws.

## Quick start

```sh
cargo install --git https://github.com/Lantharos/Mullion --package mullion-cli
cargo install --git https://github.com/Lantharos/Mullion --package mullion-service
mullion new my-app
cd my-app
bun install
cargo run
```

The generated app uses `MullionWindow::main`, loads identity/content from `Mullion.toml`,
talks to Vite on `:5173` in development, and ships a typed bridge helper from Git:

```toml
# package.json
"@lantharos/mullion": "github:Lantharos/Mullion#path:packages/mullion"
```

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

`launch_or_install` (used inside `MullionWindow::main`) contacts the shared Mullion service,
opens a small native bootstrap window when the runtime is missing, then downloads, verifies,
extracts, and builds the host before the page starts.

## Window recipes

```rust
// Conventional desktop app
MullionWindow::new().app();

// Transparent palette or launcher
MullionWindow::new().palette();

// Warm background/tray window
MullionWindow::new()
    .tray_app()
    .tray_icon(/* ... */)
    .single_instance_id("com.example.my-app");

// App-drawn chrome with titlebar + sidebar glass regions
MullionWindow::new()
    .frameless()
    .glass()
    .app_chrome(AppChrome::new(38, 260));

// Browser or document tabs that can hibernate
MullionWindow::new()
    .lifecycle_policy(mullion::MullionLifecyclePolicy::browser_tab());
```

Guests are composited OSR surfaces rather than native child webviews:

```js
const guest = await window.mullion.guest.create({
  url: "https://example.com",
  bounds: { x: 16, y: 64, width: 900, height: 600 },
  partition: "persist:browser",
  allowBridge: false,
});
```

## Runtime and service

Runtime data is stored per platform under the Mullion application-data directory. On Linux that is:

```text
~/.local/share/mullion/runtimes/cef/
```

`mullion-service` oversees the shared runtime and every registered Mullion app. By default it installs
a login/startup entry so it is ready when the computer starts. If the user disables that
(`mullion-service prefer-on-demand`), the service starts when the first Mullion app launches.

Useful commands:

```sh
mullion runtime doctor
mullion runtime install --package standard
mullion runtime prepare
mullion runtime list
mullion runtime prune --keep 2

mullion-service install
mullion-service prefer-on-demand
mullion-service prefer-login
mullion-service ensure
mullion-service ensure-runtime
mullion-service list
mullion-service maintain
mullion-service run --interval-seconds 21600
```

The long-running service checks for the latest compatible Chromium runtime, installs it atomically,
keeps running apps on their current runtime, and prunes old versions after they become unused. Apps
are registered in one atomic catalog with version, executable, update channel, and update policy.
Because Mullion is the shared system runtime, `mullion-service uninstall` also removes every
registered Mullion app and its desktop integration.

## App configuration and bundles

`Mullion.toml` is the single app manifest used by the CLI and bundle pipeline:

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

Apps can also load the same file at runtime with `MullionWindow::with_manifest(...)`, so identity
and content paths stay out of Rust while bridge handlers stay in code.

```sh
mullion install .
mullion update
mullion bundle . --target portable --release
mullion bundle . --target deb --release
mullion bundle . --target msi --release
mullion bundle . --target dmg --release
```

Mullion prefers Bun for web builds. Platform package tools are invoked only when present; otherwise
the fully staged tree and platform metadata remain in the output directory.

## Workspace

| Crate | Responsibility |
| --- | --- |
| `mullion` | Public API, OSR host, GPU renderer, Mullion host, guests, lifecycle, desktop services |
| `mullion-runtime` | Runtime discovery, verified downloads, locks, versions, paths, updates, pruning |
| `mullion-service` | Shared process, app registry, update policy, runtime maintenance, login/on-demand start |
| `mullion-bridge` | Typed commands, permissions, activity leases, guests, injected web API |
| `mullion-platform` | Native window, shell, region, tray, shortcut, and platform primitives |
| `mullion-cli` | Project creation, source installs, runtime commands, bundles |

See [the implementation guide](docs/implementation-guide.md) for the process model, protocol,
security boundaries, lifecycle, guests, bundling, and platform notes.

## Development

```sh
cargo fmt
cargo build --workspace
cargo test --workspace
cargo check --target x86_64-pc-windows-gnu --workspace
```

Mullion is dual-licensed under MIT or Apache-2.0. CEF and Chromium retain their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
