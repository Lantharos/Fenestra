# Sabine implementation guide

This document describes the current architecture and the boundaries contributors should preserve.

## Process model

A Sabine app can run in three modes from the same executable:

1. The normal app process configures `SabineWindow`, native services, bridge commands, and policy.
2. The bootstrap child shows a native progress window while the shared service is downloaded (if
   needed) and the Chromium runtime is installed. App installers ship app code plus this bootstrap so
   the first launch prepares the machine for every future Sabine app.
3. The OSR native-host child owns the winit window, wgpu renderer, input, composition, and CEF helper.

Apps enter through `SabineWindow::main`. It dispatches internal child modes, builds the app window,
launches it, and owns the process wait. `SabineWindow::launch` remains available when an application
needs to integrate Sabine into an existing native process loop.

Custom entry points that initialize an argument parser, logger, or application runtime should
dispatch Sabine's internal children before doing any of that work:

```rust
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if sabine::dispatch_host_mode_from_args(&args) {
        return;
    }

    // Parse application arguments and initialize the app here.
}
```

The root dispatcher covers both OSR-host and runtime-bootstrap children. It returns `false` for an
ordinary app invocation without initializing Sabine. `SabineWindow::main` calls it automatically.

The CEF helper is built from the C++ source embedded in the `sabine` crate. It always enables
windowless rendering and always creates `SabineOsrHandler`; there is no windowed CEF handler.

### App identity and windows

Every launch requires a non-empty `app_id` (via `.app_id(...)` or `with_manifest`). That id selects
the CEF profile directory and the private IPC directory under `$XDG_RUNTIME_DIR/sabine/<app_id>/`
(mode `0700`, sockets `0600`).

- **Different `app_id`**: separate CEF processes and profiles; no handoff between apps.
- **Same `app_id`**: windows share one CEF browser process (process-singleton handoff). Each window
  still has its own OSR host and authenticated Unix socket. Paint, input, and bridge
  request/response/events all travel on that per-window socket so invokes reach the owning app
  process. Closing one window closes only that browser; CEF quits when the last OSR handler is gone.
  Separate top-level launches therefore close independently while still being able to talk through
  the shared profile/CEF process when the app wants that.
- **Same-process multi-window**: call `SabineProcess::open_window` to spawn another OSR host with
  the same handlers and `app_id` without a second top-level `wait()` island. Use
  `close_window(WindowId)` to tear down one surface; `wait()` returns only after every OSR window
  has exited. If the parent app process exits, its OSR children are terminated; closing a child
  window does not exit the parent.
- **Second OS launch**: optional `single_instance` still focuses an existing process when enabled.
  Without it, a second launch of the same app uses CEF handoff plus a new per-window socket.
  After exit-24 handoff the secondary host waits up to 15s for the primary CEF process to connect.

OSR authentication uses a first-line token plus same-UID `SO_PEERCRED` checks on Unix. The token
is written to a `0600` file beside the socket and referenced by `--sabine-osr-token-file=` on the
CEF command line (path is not secret; this survives process-singleton handoff). `SABINE_OSR_TOKEN`
remains an optional fallback. Child OSR/CEF processes set `PR_SET_PDEATHSIG` on Linux so they do
not outlive a crashed parent.

## Paint and composition

Sabine uses one paint policy per platform. Apps cannot select a renderer or opt into experimental
transport branches.

- **Windows** uses accelerated `OnAcceleratedPaint`. CEF owns and pools the callback texture, so the
  CEF host first opens it on D3D11 and copies it into one of four Sabine-owned D3D12 shared textures
  before returning. Each destination is opened on the producer's D3D11 device for the copy and on
  wgpu's D3D12 device for composition. The host waits for its GPU copy before publishing the frame;
  the compositor sends a release acknowledgement only after its own copy completes. The producer
  never reuses a slot before that acknowledgement. Physical texture dimensions remain separate
  from the visible source rectangle and logical window dimensions at non-integer display scales.
  Mailbox saturation drops an intermediate GPU frame and requests the newest paint; it never falls
  back to a synchronous CPU readback.
- **Linux** uses CEF software `OnPaint` on Wayland. The previous DMA-BUF/X11/Vulkan branch was not a
  valid ownership implementation and has been removed.
- **macOS** currently uses software `OnPaint`. An IOSurface path must copy or retain CEF's pooled
  resource before the callback returns; passing an IOSurface ID asynchronously is not sufficient.

Windows uses wgpu D3D12 rather than forcing Vulkan. Transparent windows use a DirectComposition
visual without an HWND redirection bitmap, allowing premultiplied OSR pixels to reveal the native
backdrop. Sabine applies Acrylic, blur, Mica, and Mica Alt directly through Win32 composition APIs.
On macOS, Sabine installs its own semantic `NSVisualEffectView` beneath the Metal content view.

CEF delivers BGRA dirty rectangles on the software path. Inline and shared-memory batches retain one
immutable byte backing instead of copying every rectangle into a separate allocation. The native
host patches one backing store per surface and uploads only the changed ranges to an independent GPU
texture (sparse per-rect uploads when damage is disjoint):

- main page
- popup overlay
- one texture per guest (including guest `<select>` popups)

The display list damages the union of the old and new bounds when a primitive changes. GPU vertex
buffers grow geometrically and are reused across redraws. Native surface resize is presented
synchronously with the last frame at its original logical size while CEF catches up. Resize control
messages coalesce to the newest size on CEF's UI thread. Main paints must exactly match that logical
size, and accelerated paints with transitional coded/content/source metadata are discarded. This
keeps the swapchain responsive without stretching a stale frame or presenting a partially relaid-out
Chromium texture.

Transport is platform-specific without changing the protocol. On Unix, sockets live under
`$XDG_RUNTIME_DIR/sabine/<app_id>/` (mode `0700`) with socket mode `0600`, and each window
authenticates with a first-line token read from a one-use `0600` token file. The environment is
only a fallback for launches that do not need Chromium process-singleton handoff.

Paint messages use the versioned `SAB1` wire signature. Surface dimensions, inline payloads, and
shared mappings are bounded before allocation or mapping, and the native host uses a bounded event
queue so a stalled compositor applies backpressure instead of accumulating frames indefinitely.

| Platform | Transport | Paint path |
| --- | --- | --- |
| Linux | Unix domain socket | dirty-rect BGRA `OnPaint` → sparse wgpu uploads |
| macOS | Unix domain socket | dirty-rect BGRA `OnPaint` → sparse wgpu uploads |
| Windows | localhost TCP | CEF D3D11 → acknowledged NT texture slots → wgpu D3D12 |

Linux layer-shell surfaces use their dedicated host because layer-shell configuration must happen
before a regular winit surface is created. Other platforms express palette behavior with a normal
frameless, always-on-top, hide-on-blur window.

## Runtime ownership

`sabine-runtime` is the only crate allowed to decide runtime locations, versions, download archives,
integrity, install locks, and pruning. The runtime is always CEF. WebView2 is not an alternate backend or a
fallback.

An install has these phases:

```text
plan -> lock -> download -> SHA-1 verify -> extract -> atomic version install -> ready
```

The Spotify CEF index supplies archive metadata and checksums. Runtime versions are immutable
directories, allowing existing apps to finish on an older version while the service installs a newer
one. Runtime and CEF-host builds have independent stale-aware locks. Detection validates the exact
package and current platform: a Standard runtime requires its SDK tree, the platform CEF binary,
`icudtl.dat`, `resources.pak`, and at least one locale pack. Partial extraction directories are never
adopted or reused.

`sabine-service` owns the machine/user-level catalog. Its registry writes use a temporary file,
`sync_all`, and atomic rename. Re-registering an app preserves its original registration timestamp.
The dedicated `sabine-service-daemon` owns its PID file and maintenance loop. Linux starts it with a
user systemd unit, macOS with a LaunchAgent, and Windows with a hidden per-user scheduled task; the
Windows binary uses the GUI subsystem and never creates a console host. The maintenance loop updates
CEF to the newest compatible archive and keeps two runtime versions.

## Public API

The primary API is a fluent `SabineWindow` builder. Configuration is grouped by concern even though
the builder keeps common cases one method away:

- content: local entry, production URL, or a dev URL already owned by the CLI
- window: size, chrome, visibility, transparency, blur and control regions
- browser: Chromium flags, devtools, profiles and security
- bridge: descriptors, sync handlers and async handlers
- lifecycle: foreground/background rates, suspend, hibernate and memory-saver policy
- services: tray, autostart, shortcuts, deep links, native messaging and single instance
- runtime: minimum version and bundled/shared policy

The API deliberately avoids backend types. Apps do not select paint transports or Chromium launch
flags. The primary surface is `SabineWindow`, its recipes, typed background effects and regions,
bridge handlers, lifecycle policy, desktop integrations, and `RuntimeConfig`. Runtime maintenance,
service policy, and native-host build helpers stay in their owning crates instead of being re-exported
through `sabine`.

## Bridge security

Bridge commands must be registered before launch. Each command can constrain targets and origins.
The host rejects unknown commands, invalid targets, and origins outside the configured allowlist.

Local `file://` application content is trusted by default. Remote content is not implicitly trusted.
Calling `.url(...)` adds that URL's exact origin; development URLs add loopback variants needed by
local toolchains.

Guests default to `allow_bridge = false`. A guest gets an isolated request context when it declares a
partition. Popup policy, download policy, visibility, bounds, intercepted shortcuts, and horizontal
wheel interception are all explicit guest properties.

While a guest is focused, matching `interceptedShortcuts` are consumed by the host: keydown emits
`guest.shortcut` to the primary page and neither keydown nor keyup reaches the guest. With
`interceptHorizontalWheel`, predominantly horizontal wheel samples emit `guest.wheel` and are not
forwarded to the guest. Favicon URL changes emit `guest.favicon`.

HTML file drags from the primary page are promoted to native OS drags. Incoming URI-list drags,
including self-drops, are accepted by the native host and emitted to the primary page as
`window.fileDrag`. Each event carries its phase, absolute file paths, content coordinates, the
negotiated copy/move/link action, and whether it originated from the same Sabine window. Apps use
those coordinates to resolve their own semantic drop targets without exposing renderer internals to
the native host.

The web bridge exposes:

```text
window.sabine.bridge
window.sabine.window
window.sabine.lifecycle
window.sabine.activity
window.sabine.guest
```

## Lifecycle and performance

Sabine distinguishes active, background, suspended, hibernating, and hibernated states. Activity
leases allow durable Rust work or page work to block hibernation while it is genuinely active.
Blur and occlusion suspend only lower the windowless frame rate; they do not call CEF `WasHidden`,
so brief focus loss during interactive move does not blank the surface. Hibernation is what actually
hides the view and tears down the renderer.

Memory saver is explicit. `SabineLifecyclePolicy::memory_saver_hidden_window()` or
`.memory_saver(true)` enables the aggressive hidden-window policy and prevents Chromium from
keeping a spare renderer warm. Normal browser-tab and hidden-window policies retain Chromium's
spare renderer so future navigations do not pay an avoidable process-start penalty.

Rules for renderer changes:

- never replace damage tracking with unconditional full-frame uploads
- retain the last frame only when the selected lifecycle policy asks for it
- keep hidden palette windows warm unless memory-saver policy opts into hibernation
- do not poll when a native event or deadline can wake the event loop
- keep bridge and paint traffic off the UI thread except for final state application

## Desktop services

Desktop service configuration belongs to the window so an app has one declarative startup surface.
Implementations must degrade by reporting unsupported capabilities, not by silently changing the app
model.

Current primitives cover tray menus, autostart, global shortcuts, deep links, native messaging,
single-instance activation, hidden windows, always-on-top windows, and palette behavior. Native
platform registration belongs in `sabine-platform` or `sabine-service`; CEF code must not own it.

## Bundles and installs

The CLI reads `Sabine.toml`, builds web assets, builds the selected Rust package, stages the runtime
layout, writes platform metadata, and invokes a local package tool when available.

Supported targets are portable, Linux directory, deb, rpm, AppImage, Windows directory, exe, msi,
macOS app, and dmg. Cross-host staging is allowed; signing and notarization remain deployment policy.

Source installs are development conveniences. They stage assets and a launcher under the Sabine data
directory, register the app with the service, and create platform launch metadata.

## Validation

After code changes run:

```sh
cargo fmt
cargo build --workspace
cargo test --workspace
cargo check --target x86_64-pc-windows-gnu --workspace
```

CI enforces formatting, warning-free Clippy, and workspace tests on Linux, then runs the workspace
tests natively on Windows MSVC and macOS as well.

The Windows cross-check validates Rust cfg coverage. A release still needs a native Windows CEF-host
build and an actual GPU/input smoke test. The same rule applies to macOS. Linux tests do not prove
Windows or macOS composition behavior.

On Windows, `sabine-host` must be built with **MSVC** (Visual Studio 2019+ C++ workload). Official CEF
binaries do not link with MinGW/MSYS. Sabine forces a Visual Studio CMake generator and normalizes
`CEF_ROOT` to forward slashes so CEF’s cmake macros do not treat `\Users\...` as escape sequences.

CEF archives are extracted with `tar` using the destination as the process working directory (not
`tar -C C:\...`). Git for Windows’ GNU tar treats a drive letter in `-C` as a remote host and can
leave a partial tree that looks installed but cannot build the host.
