use std::path::{Path, PathBuf};

use crate::types::RuntimePackage;

pub fn has_host_binary(path: &Path) -> bool {
    launchable_host_candidates(path)
        .into_iter()
        .any(|candidate| candidate.is_file())
}

pub fn host_candidates(runtime_dir: &Path) -> Vec<PathBuf> {
    launchable_host_candidates(runtime_dir)
}

pub fn launchable_host_candidates(runtime_dir: &Path) -> Vec<PathBuf> {
    vec![
        runtime_dir.join("cefclient"),
        runtime_dir.join("Release").join("cefclient"),
        runtime_dir.join("bin").join("cefclient"),
        runtime_dir.join("cefsimple"),
        runtime_dir.join("Release").join("cefsimple"),
        runtime_dir.join("cefclient.exe"),
        runtime_dir.join("Release").join("cefclient.exe"),
        runtime_dir.join("cefsimple.exe"),
        runtime_dir.join("Release").join("cefsimple.exe"),
        runtime_dir
            .join("cefclient.app")
            .join("Contents")
            .join("MacOS")
            .join("cefclient"),
        runtime_dir
            .join("cefsimple.app")
            .join("Contents")
            .join("MacOS")
            .join("cefsimple"),
    ]
}

pub(crate) fn runtime_is_launchable_client(runtime_dir: &Path) -> bool {
    runtime_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(RuntimePackage::Client.install_suffix()))
        && has_host_binary(runtime_dir)
}

pub(crate) fn runtime_is_standard(runtime_dir: &Path) -> bool {
    runtime_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(RuntimePackage::Standard.install_suffix()))
        && runtime_dir.join("include").is_dir()
        && runtime_dir.join("libcef_dll").is_dir()
        && has_libcef_binary(runtime_dir)
}

fn has_libcef_binary(runtime_dir: &Path) -> bool {
    let release = runtime_dir.join("Release");
    release.join("libcef.so").is_file()
        || release.join("libcef.dll").is_file()
        || release.join("libcef.dylib").is_file()
        || release
            .join("Chromium Embedded Framework.framework")
            .is_dir()
        || runtime_dir
            .join("Chromium Embedded Framework.framework")
            .is_dir()
}
