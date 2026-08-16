use std::path::Path;

pub(crate) fn detect_version(runtime_dir: &Path) -> String {
    let version_file = runtime_dir.join(".sabine-version");
    std::fs::read_to_string(version_file)
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

pub(crate) fn version_sort_key(version: &str) -> Vec<u32> {
    version
        .split(['.', '+', '-', '_'])
        .filter_map(|part| part.parse::<u32>().ok())
        .collect()
}

pub(crate) fn channel_preference(channel: Option<&str>) -> u8 {
    match channel {
        Some("stable") | None => 0,
        Some("beta") => 1,
        Some("dev") | Some("canary") => 2,
        _ => 3,
    }
}

pub(crate) fn major_version(version: &str) -> u32 {
    version
        .split(['.', '+'])
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(0)
}

pub(crate) fn version_satisfies(found: &str, required: &str) -> bool {
    found != "unknown" && major_version(found) >= major_version(required)
}

pub(crate) fn runtime_sort_key(path: &Path) -> Vec<u32> {
    version_sort_key(&detect_version(path))
}

pub(crate) fn cef_platform_key() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("linux64"),
        ("linux", "aarch64") => Some("linuxarm64"),
        ("windows", "x86_64") => Some("windows64"),
        ("windows", "aarch64") => Some("windowsarm64"),
        ("macos", "x86_64") => Some("macosx64"),
        ("macos", "aarch64") => Some("macosarm64"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::detect_version;

    #[test]
    fn runtime_marker_does_not_use_cef_version_header_name() {
        let root =
            std::env::temp_dir().join(format!("sabine-version-marker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("version"), "CEF C++ header").unwrap();
        std::fs::write(root.join(".sabine-version"), "151.3.18").unwrap();
        assert_eq!(detect_version(&root), "151.3.18");
        assert_eq!(
            std::fs::read_to_string(root.join("version")).unwrap(),
            "CEF C++ header"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
