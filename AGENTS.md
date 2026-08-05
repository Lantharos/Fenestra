# Agent Instructions

- Use `bun` for JavaScript package management.
- Mullion runtime detection, downloads, validation, manifests, locks, pruning, and runtime paths live in `crates/mullion-runtime`.
- Shared app registration, runtime maintenance, and update policy live in `crates/mullion-service`.
- CEF host source, CEF process launch, browser profiles, OSR transport, GPU composition, guests, and the public window API live in `crates/mullion`.
- Inside `crates/mullion/src`, domain folders are: `window/` (builder/config/glass), `desktop/` (tray/shortcuts/autostart), `launch/` (host args/bootstrap), `host/` (CEF host binary + process handles), `bridge/` (host-side IPC wiring), `osr/` (protocol, desktop host, Wayland layer host), and `render/`.
- Mullion uses OSR on every desktop. Do not add windowed CEF, WebView2, or another renderer fallback.
- `mullion-platform` owns lightweight window/platform/shell types, compositor regions, and native platform primitives.
- Apps should use `MullionWindow` from `mullion` directly.
- Prefer `MullionWindow::main`, recipes (`.app()` / `.palette()` / `.tray_app()`), `AppChrome`, and `with_manifest` for new apps; keep advanced shell/region APIs available but secondary.
- Run `cargo fmt`, `cargo build --workspace`, and `cargo test --workspace` after code changes. For Windows-only changes, also run `cargo check --target x86_64-pc-windows-gnu --workspace` since the host development environment is typically Linux.
- When publishing, use `scripts/publish.sh`. Crate metadata lives in each crate's `Cargo.toml`; the workspace owns version, license, repository, homepage, authors, keywords, categories.
