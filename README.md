![Mullion](assets/banner.png)

# Mullion

Mullion is a native application framework built around one shared Chromium runtime. Every desktop
uses the same CEF off-screen-rendering pipeline, the same bridge, the same guest model, and the same
runtime service. Apps keep native windows, GPU composition, tray and palette behavior, filesystem
access, background work, and platform integration without shipping a browser engine per app.

The project is early and intentionally fast-moving; pre-1.0 APIs may change when that improves the design.

## Why Mullion

- One CEF runtime across Linux, Windows, and macOS
- OSR-only rendering; no WebView2 backend and no windowed CEF fallback
- Damage-driven GPU composition with dirty-rect uploads
- Embedded guests for browser tabs, previews, authentication, and untrusted pages
- Typed Rust-to-web bridge with explicit command and origin permissions
- Native first-run runtime installer before any web content starts
- Shared service for app registration, CEF updates, pruning, and update policy
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
        |      +-- CEF helper process
        |      +-- main page and guest surfaces
        |      +-- damage-only texture uploads
        |
        +-- mullion-service
               +-- shared CEF runtime
               +-- registered app catalog
               +-- runtime updates and pruning
               +-- app update channels and policy
```

Linux uses a Unix socket and shared-memory paint batches for large updates. macOS uses the same local
Unix transport and falls back to inline dirty rectangles when shared buffers are unavailable. Windows
uses authenticated loopback transport and inline dirty rectangles. None of these paths
switches to full-frame redraws.

## Quick start

Install the CLI:

```sh
cargo install --git https://github.com/Misoworks/Mullion --package mullion-cli
```

Create and run an app:

```sh
mullion new my-app
cd my-app
cargo run
```

The generated host handles Mullion child modes before creating the app window:

```rust
use mullion::{BridgeResponse, MullionWindow, run_mullion_host_from_args};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if run_mullion_host_from_args(&args) {
        return Ok(());
    }

    MullionWindow::new()
        .app_id("com.example.my-app")
        .title("My App")
        .entry("ui/index.html")
        .bridge_handler("app.version", |_| {
            Ok(BridgeResponse::json(serde_json::json!({ "version": "1.0.0" })))
        })
        .launch_or_install()?
        .wait()?;
    Ok(())
}
```

`launch_or_install` opens a small native bootstrap window when the shared runtime is missing. The
download, verification, extraction, and host build happen before the page is allowed to start.

## Window recipes

```rust
// Conventional desktop app
MullionWindow::new().system_chrome();

// App-drawn titlebar
MullionWindow::new().mullion_chrome();

// Transparent palette or launcher
MullionWindow::new()
    .frameless()
    .glass()
    .hide_on_blur(true);

// Warm background/tray window
MullionWindow::new()
    .hidden()
    .lifecycle_policy(mullion::MullionLifecyclePolicy::hidden_window());

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

Useful commands:

```sh
mullion runtime doctor
mullion runtime install --package standard
mullion runtime prepare
mullion runtime list
mullion runtime prune --keep 2

mullion-service ensure-runtime
mullion-service list
mullion-service maintain
mullion-service run --interval-seconds 21600
```

The long-running service checks for the latest compatible CEF build, installs it atomically, keeps
running apps on their current runtime, and prunes old versions after they become unused. Apps are
registered in one atomic catalog with version, executable, update channel, and update policy.
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
build = "bun run build"
```

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
| `mullion` | Public API, OSR host, GPU renderer, CEF helper, guests, lifecycle, desktop services |
| `mullion-runtime` | CEF discovery, verified downloads, locks, versions, paths, updates, pruning |
| `mullion-service` | Shared process, app registry, update policy, runtime maintenance |
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
