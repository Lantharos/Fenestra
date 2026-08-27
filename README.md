![Sabine](assets/banner.png)

# Sabine

Sabine is a native application framework built around one shared Chromium runtime.
Write your UI in the web stack you already use, keep a real desktop window, and share one
browser engine across every Sabine app on the machine.

## Install

```toml
[dependencies]
sabine = { git = "https://github.com/Lantharos/Sabine", tag = "v0.1.20" }
```

```sh
cargo install --git https://github.com/Lantharos/Sabine --tag v0.1.20 sabine-cli
```

For the TypeScript helpers used by the web UI:

```sh
bun add github:Lantharos/Sabine#v0.1.20
```

## Why Sabine

- One shared Chromium runtime across Linux, Windows, and macOS
- Native windows with GPU composition, glass materials, trays, and palettes
- Guests for embedded tabs, previews, auth flows, and untrusted pages
- Typed Rust ↔ web bridge with explicit command and origin permissions
- Shared service that owns first-run setup, the Chromium runtime, and future tools
- Visible progress while the first app prepares the machine for every Sabine app
- One `Sabine.toml` for app identity, web assets, and packaging
- Lifecycle controls for background windows, tray apps, and browser-style workloads
- TypeScript package for invoke, guests, window controls, native file drops, activity, and popups

## Quick start

```sh
sabine new my-app
cd my-app
sabine dev
```

Generated apps look like this:

```rust
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use sabine::prelude::*;
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
            .background_color(SabineColor::rgb8(15, 17, 21))
            .size(960, 640)
            .bridge_typed("app.version", |_request: VersionRequest| {
                Ok(VersionResponse {
                    version: env!("CARGO_PKG_VERSION"),
                })
            }))
    });
}
```

```js
import { invoke, guest, appWindow, listen, events } from "@lantharos/sabine";

const { version } = await invoke("app.version");
listen("tray.click", () => appWindow.show());
events.fileDrag(({ phase, paths, x, y, action }) => {
  console.log(phase, paths, x, y, action);
});

const tab = await guest.create({
  url: "https://example.com",
  bounds: { x: 16, y: 64, width: 900, height: 600 },
  partition: "persist:browser",
});
```

On first launch (or `sabine dev`), Sabine prepares and validates the shared Chromium runtime when it
is missing. App launches register with the shared Sabine service and later runs reuse the verified
install. `sabine dev` owns the web development process; the app only connects to the configured
development URL.

## Window recipes

```rust
// Standard desktop app
SabineWindow::new().app();

// Transparent palette or launcher
SabineWindow::new().palette();

// Background tray app
SabineWindow::new()
    .tray_app()
    .tray_icon(/* ... */)
    .single_instance_id("com.example.my-app");

// Custom titlebar and sidebar glass regions
SabineWindow::new()
    .frameless()
    .glass()
    .app_chrome(AppChrome::new(38, 260));
```

Embed another page as a guest surface:

```js
import { guest } from "@lantharos/sabine";

const surface = await guest.create({
  url: "https://example.com",
  bounds: { x: 16, y: 64, width: 900, height: 600 },
  partition: "persist:browser",
  allowBridge: false,
});
await surface.setBounds({ x: 16, y: 64, width: 1100, height: 700 });
```

## Configuration

`Sabine.toml` describes the app and its web assets. `SabineWindow::main` loads it automatically;
the CLI supplies its source location during development and writes a relocated production manifest
when packaging:

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
dev_port = 5173
build = "bun run build"

[updates]
provider = "github"
repository = "your-name/my-app"
channel = "stable"
policy = "automatic"
```

```sh
sabine install .
sabine update
sabine bundle . --target portable --release
sabine bundle . --target deb --release
sabine bundle . --target msi --release
sabine bundle . --target dmg --release
```

## Publishing and updates

Sabine uses one coordinated `vX.Y.Z` GitHub Release for the CLI, service, daemon, and prebuilt CEF
host. The release workflow builds the complete system bundle for each platform and signs one
immutable release manifest. Bootstrap verifies the Ed25519 signature and artifact SHA-256 before
installing the binaries side by side. Customer machines do not compile Sabine or require Rust,
CMake, or a C++ toolchain. CEF remains a separately managed runtime and apps never select a
Chromium version.

New apps include a release workflow that calls Sabine's reusable workflow. Run the one-time release
setup from the app repository; it creates the app's signing key, stores the private seed directly as
a GitHub Actions secret, enables immutable Releases, and writes the repository and public key to
`Sabine.toml`:

```sh
sabine release-init --repository owner/repository
```

For each release, set the version in `Sabine.toml`, commit it, and push the matching tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The workflow builds MSI, DMG, AppImage, deb, and rpm artifacts, signs `sabine-update.json`, attaches
the exact platform/package mapping, attests the artifacts, and creates the GitHub Release. The daemon
downloads updates in the background. Sabine-managed archives activate side by side; their stable
bootstrap forwards to the current release on the next launch. Native packages are staged silently,
then offer Install or Later when the app next opens. Accepting closes the app, requests elevation
when needed, installs, and relaunches it. Store installs remain owned by the store.

Windows bundles statically link the Microsoft C runtime. MSI packages are x64, install under
`Program Files`, and use the configured app icon for their Start menu shortcut. The release workflow
installs and launches the MSI before publishing it. Windows bundle targets are linked as GUI apps,
so existing apps do not need source-level linker configuration to avoid a console window.
Sabine's own service and daemon are also self-contained, and bootstrap tools run without creating
console windows; the native setup progress window is the only visible first-launch process.

Pass `--offline` to `sabine bundle` to include a working CEF runtime and Sabine system bootstrap.
The embedded system is adopted into the same versioned installation on first launch, then resumes
normal background updates. No compiler is needed on the destination machine.

## Runtime and service

Sabine keeps the Chromium runtime under the platform application-data directory. On Linux:

```text
~/.local/share/sabine/runtimes/cef/
```

`sabine-service` owns machine setup for every Sabine app. A separate
`sabine-service-daemon` executable performs background maintenance so Windows never attaches a
console window to the login process.

1. The first app launches with Sabine bootstrap code and a native progress window.
2. Bootstrap downloads the signed service, daemon, and prebuilt CEF host system bundle from the
   latest Sabine GitHub Release, installs it in a versioned directory, then starts it.
3. The service installs the latest compatible Chromium runtime and validates it with a headless CEF
   initialization before selecting it, independent of the daemon's graphical-session environment.
4. The app registers with the service and starts.

Later apps reuse that service and runtime. Updates are atomic: Sabine retains the previous system
version until the replacement daemon reports healthy, and CEF runtimes in active use hold leases so
maintenance cannot prune them. A failed CEF initialization is quarantined and resolution falls back
to the previous runtime. Quarantines are scoped to the health-probe version so a corrected probe
automatically reconsiders runtimes it previously rejected. By default the service also starts at
login so the runtime stays warm.
Prefer on-demand start with `sabine-service prefer-on-demand`.

Service acquisition order:

1. `SABINE_SERVICE_PATH` if set
2. Active versioned installation under the Sabine data dir
3. Complete offline bootstrap beside the app, adopted into the versioned installation
4. Binary on `PATH` for development
5. The platform system bundle described by
   `https://github.com/Lantharos/Sabine/releases/latest/download/sabine-release.json`

Override the release metadata URL with `SABINE_RELEASE_MANIFEST_URL` for development or a private
mirror.

```sh
sabine runtime doctor
sabine runtime install
sabine runtime list
sabine runtime prune --keep 2

sabine-service install
sabine-service ensure
sabine-service list
sabine-service maintain
```

`sabine runtime doctor` validates the runtime layout and launches the matching CEF host with a
headless smoke probe. Its JSON output includes `probe_error` when the host cannot start.

## Learn more

- [Implementation guide](docs/implementation-guide.md) — process model, bridge, guests, bundling, and platform notes
- [`@lantharos/sabine`](packages/sabine) — TypeScript helpers for the page bridge

## License

Sabine is dual-licensed under MIT or Apache-2.0. CEF and Chromium keep their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
