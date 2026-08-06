# Sabine implementation guide

This document describes the current architecture and the boundaries contributors should preserve.

## Process model

A Sabine app can run in three modes from the same executable:

1. The normal app process configures `SabineWindow`, native services, bridge commands, and policy.
2. The bootstrap child shows a native progress window while the shared service is downloaded (if
   needed) and the Chromium runtime is installed. App installers ship app code plus this bootstrap so
   the first launch prepares the machine for every future Sabine app.
3. The OSR native-host child owns the winit window, wgpu renderer, input, composition, and CEF helper.

Every app must call `run_sabine_host_from_args` before constructing its normal application state.
This dispatches internal child modes without requiring an additional app-specific executable.

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

Accelerated OSR is preferred on Windows and macOS. On Linux, software `OnPaint` is the default
because Chromium's shared-texture (DMA-BUF) path still fails SkSurface initialization on many
drivers — especially NVIDIA. Opt in with `WindowBuilder::shared_texture_osr(true)` /
`--sabine-shared-texture`. Software is also forced with `--sabine-software-osr`.

Linux CEF ozone defaults to **Wayland** (same as the Sabine shell). Shared-texture opt-in
still forces **X11** ozone + ANGLE `gl-egl` because that is what CEF/Chromium currently require
for DMA-BUF OSR — not because Sabine wants X11.

- **Linux** — when shared textures are enabled, the host sends DMA-BUF file descriptors for
  `OnAcceleratedPaint`; the compositor imports them zero-copy into wgpu (Vulkan external memory)
  when the adapter supports it, otherwise maps the plane into the existing dirty-rect framebuffer
  path.
- **Windows** — the host duplicates the D3D11 shared `HANDLE` into the compositor process; wgpu
  imports it on a Vulkan device when `VULKAN_EXTERNAL_MEMORY_WIN32` is available.
- **macOS** — the host sends the `IOSurfaceID` for `OnAcceleratedPaint`; the compositor looks
  it up and wraps a Metal texture into wgpu. If IPC fails, the host locks the surface and bridges
  BGRA into the paint path.

If accelerated paint cannot run, Sabine silently relaunches the CEF helper with software `OnPaint`
— there is no public paint-mode switch or CEF handle exposure.

CEF still delivers BGRA dirty rectangles on the software path (and as a CPU bridge after GPU
raster). The native host retains a backing store per surface and patches only the changed ranges.
Each surface is then uploaded to an independent GPU texture (sparse per-rect uploads when damage is
disjoint):

- main page
- popup overlay
- one texture per guest (including guest `<select>` popups)

The display list damages the union of the old and new bounds when a primitive changes. Resize waits
for a correctly sized CEF paint rather than stretching a stale frame indefinitely.

Transport is platform-specific without changing the protocol. On Unix, sockets live under
`$XDG_RUNTIME_DIR/sabine/<app_id>/` (mode `0700`) with socket mode `0600`, and each window
authenticates with a first-line token delivered via `SABINE_OSR_TOKEN` (not argv).

| Platform | Transport | Accelerated paint path |
| --- | --- | --- |
| Linux | Unix domain socket | DMA-BUF FD + Vulkan → wgpu (mmap fallback) |
| macOS | Unix domain socket | IOSurfaceID → Metal → wgpu (BGRA bridge if IPC fails) |
| Windows | localhost TCP | duplicated D3D11 HANDLE → Vulkan → wgpu |

Software `OnPaint` dirty-rect batches remain the silent fallback when accelerated paint cannot run.

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
one. Runtime and CEF-host builds have independent stale-aware locks.

`sabine-service` owns the machine/user-level catalog. Its registry writes use a temporary file,
`sync_all`, and atomic rename. Re-registering an app preserves its original registration timestamp.
The maintenance loop updates CEF to the newest compatible archive and keeps two runtime versions.

## Public API

The primary API is a fluent `SabineWindow` builder. Configuration is grouped by concern even though
the builder keeps common cases one method away:

- content: local entry, production URL, dev URL and command
- window: size, chrome, visibility, transparency, blur and control regions
- browser: Chromium flags, devtools, profiles and security
- bridge: descriptors, sync handlers and async handlers
- lifecycle: foreground/background rates, suspend and hibernate policy
- services: tray, autostart, shortcuts, deep links, native messaging and single instance
- runtime: package, minimum version, bundled/shared policy

The API deliberately avoids backend types. `BrowserOptions` contains browser-process tuning; apps do
not select a renderer. Sabine always uses the shared OSR host.

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

The Windows cross-check validates Rust cfg coverage. A release still needs a native Windows CEF-host
build and an actual GPU/input smoke test. The same rule applies to macOS. Linux tests do not prove
Windows or macOS composition behavior.
