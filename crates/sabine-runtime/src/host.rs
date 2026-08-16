use std::path::{Path, PathBuf};

pub(crate) fn runtime_is_standard(runtime_dir: &Path) -> bool {
    standard_sdk_present(runtime_dir)
}

/// Headers + CMake modules needed to build `sabine-host` against a Standard CEF tree.
pub(crate) fn standard_sdk_present(runtime_dir: &Path) -> bool {
    runtime_dir.join("cmake").is_dir()
        && runtime_dir.join("include").is_dir()
        && runtime_dir.join("libcef_dll").is_dir()
        && runtime_dir.join("include").join("cef_version.h").is_file()
        && has_libcef_binary(runtime_dir)
        && runtime_resources_present(runtime_dir)
}

fn runtime_resources_present(runtime_dir: &Path) -> bool {
    resource_roots(runtime_dir).into_iter().any(|root| {
        root.join("icudtl.dat").is_file()
            && root.join("resources.pak").is_file()
            && locales_present(&root.join("locales"))
    })
}

fn has_libcef_binary(runtime_dir: &Path) -> bool {
    let release = runtime_dir.join("Release");
    #[cfg(target_os = "linux")]
    return release.join("libcef.so").is_file();
    #[cfg(target_os = "windows")]
    return release.join("libcef.dll").is_file();
    #[cfg(target_os = "macos")]
    return [runtime_dir.to_path_buf(), release]
        .into_iter()
        .any(|root| {
            root.join("Chromium Embedded Framework.framework")
                .join("Chromium Embedded Framework")
                .is_file()
        });
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    false
}

fn resource_roots(runtime_dir: &Path) -> Vec<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    return vec![runtime_dir.join("Resources"), runtime_dir.join("Release")];
    #[cfg(target_os = "macos")]
    return [runtime_dir.to_path_buf(), runtime_dir.join("Release")]
        .into_iter()
        .map(|root| {
            root.join("Chromium Embedded Framework.framework")
                .join("Resources")
        })
        .collect();
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Vec::new()
}

fn locales_present(path: &Path) -> bool {
    path.is_dir()
        && std::fs::read_dir(path).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry.path().is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "pak")
            })
        })
}

pub(crate) fn runtime_is_valid(runtime_dir: &Path) -> bool {
    !runtime_dir.join(".sabine-unusable").exists() && runtime_is_standard(runtime_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sabine-runtime-{name}-{}", std::process::id()))
    }

    #[test]
    fn rejects_partial_standard_tree() {
        let root = scratch("partial");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Release")).unwrap();
        std::fs::create_dir_all(root.join("Resources")).unwrap();
        assert!(!runtime_is_valid(&root));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn accepts_complete_standard_tree() {
        let root = scratch("complete");
        let _ = std::fs::remove_dir_all(&root);
        for directory in [
            "cmake",
            "include",
            "libcef_dll",
            "Release",
            "Resources/locales",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        for file in [
            "include/cef_version.h",
            "Resources/icudtl.dat",
            "Resources/resources.pak",
            "Resources/locales/en-US.pak",
        ] {
            std::fs::write(root.join(file), []).unwrap();
        }
        #[cfg(target_os = "linux")]
        std::fs::write(root.join("Release/libcef.so"), []).unwrap();
        #[cfg(target_os = "windows")]
        std::fs::write(root.join("Release/libcef.dll"), []).unwrap();
        assert!(runtime_is_valid(&root));
        std::fs::remove_dir_all(root).unwrap();
    }
}
