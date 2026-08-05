use std::path::{Path, PathBuf};

use crate::host::{runtime_is_launchable_client, runtime_is_standard};
use crate::paths::{bundled_runtime_path, system_runtime_path, user_runtime_path};
use crate::types::{RuntimeConfig, RuntimeInfo, RuntimeLocation, RuntimePackage};
use crate::version::detect_version;

#[derive(Clone, Copy)]
pub(crate) enum RuntimeLocationKind {
    System,
    UserLocal,
    Bundled,
}

pub fn detect_runtime(config: &RuntimeConfig) -> Vec<RuntimeInfo> {
    let mut runtimes = Vec::new();
    collect_runtime_dirs(
        RuntimeLocationKind::System,
        system_runtime_path(),
        &mut runtimes,
    );
    collect_runtime_dirs(
        RuntimeLocationKind::UserLocal,
        user_runtime_path(),
        &mut runtimes,
    );
    if config.allow_bundled
        && let Some(dir) = &config.bundled_dir
    {
        collect_runtime_dirs(
            RuntimeLocationKind::Bundled,
            bundled_runtime_path(dir),
            &mut runtimes,
        );
    }
    runtimes
}

pub(crate) fn collect_runtime_dirs(
    kind: RuntimeLocationKind,
    base: PathBuf,
    runtimes: &mut Vec<RuntimeInfo>,
) {
    if !base.is_dir() {
        return;
    }

    if is_runtime_dir(&base) {
        runtimes.push(runtime_info(kind, base));
        return;
    }

    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && is_runtime_dir(&path) {
                runtimes.push(runtime_info(kind, path));
            }
        }
    }
}

pub(crate) fn runtime_info(kind: RuntimeLocationKind, path: PathBuf) -> RuntimeInfo {
    let location = match kind {
        RuntimeLocationKind::System => RuntimeLocation::System(path.clone()),
        RuntimeLocationKind::UserLocal => RuntimeLocation::UserLocal(path.clone()),
        RuntimeLocationKind::Bundled => RuntimeLocation::Bundled(path.clone()),
    };
    RuntimeInfo {
        package: detect_package(&path),
        version: detect_version(&path),
        location,
        verified: is_runtime_dir(&path),
    }
}

pub(crate) fn detect_package(runtime_dir: &Path) -> RuntimePackage {
    if runtime_is_standard(runtime_dir) {
        RuntimePackage::Standard
    } else if runtime_is_launchable_client(runtime_dir) {
        RuntimePackage::Client
    } else {
        RuntimePackage::Minimal
    }
}

pub(crate) fn is_runtime_dir(path: &Path) -> bool {
    path.join("VERSION").is_file()
        || path.join("Release").is_dir()
        || path.join("Resources").is_dir()
        || path.join("libcef.so").is_file()
        || path.join("libcef.dll").is_file()
        || path.join("Chromium Embedded Framework.framework").is_dir()
}
