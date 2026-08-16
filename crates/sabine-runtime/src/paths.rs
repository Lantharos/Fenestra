use std::path::{Path, PathBuf};

pub fn system_runtime_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
            .join("Sabine/Runtime/cef")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/Sabine/runtimes/cef")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        PathBuf::from("/usr/lib/sabine/cef")
    }
}

pub fn user_runtime_path() -> PathBuf {
    user_data_dir().join("sabine").join("runtimes").join("cef")
}

fn user_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local);
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            return PathBuf::from(profile).join("AppData").join("Local");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support");
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| std::ffi::OsString::from("/tmp"));
    PathBuf::from(home).join(".local").join("share")
}

pub fn bundled_runtime_path(app_dir: &Path) -> PathBuf {
    app_dir.join("runtimes").join("cef")
}

pub fn runtime_version_path(version: &str) -> PathBuf {
    user_runtime_path().join(format!("{version}-standard"))
}
