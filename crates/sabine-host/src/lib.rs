//! CEF host embedder sources and cmake build.
//!
//! Apps and the CLI call [`ensure_host`] to materialize `sabine-host` next to
//! a CEF runtime. Process launch and window wiring stay in the `sabine` crate.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

mod build_lock;
mod sources;
mod toolchain;

use build_lock::HostBuildLock;
use sources::write_host_source;
use toolchain::apply_cmake_generator;

const RUNTIME_PROBE_VERSION: u32 = 2;

pub fn host_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sabine-host.exe"
    } else {
        "sabine-host"
    }
}

pub fn host_release_binary(runtime_dir: &Path) -> PathBuf {
    let root = runtime_dir
        .join(".sabine-hosts")
        .join(host_source_fingerprint());
    if cfg!(target_os = "macos") {
        root.join("sabine-host.app/Contents/MacOS/sabine-host")
    } else {
        root.join(host_binary_name())
    }
}

pub fn ensure_host(runtime_dir: &Path) -> Result<PathBuf, String> {
    if let Some(host) = available_host(runtime_dir) {
        return Ok(host);
    }
    let binary = host_release_binary(runtime_dir);
    let expected_stamp = host_source_fingerprint();
    let work_dir = runtime_dir.join(".sabine-host-build").join(&expected_stamp);
    let source_dir = work_dir.join("src");
    let build_dir = work_dir.join("build");
    if binary.is_file() {
        return Ok(binary);
    }
    let _lock = HostBuildLock::acquire(runtime_dir)?;
    if binary.is_file() {
        return Ok(binary);
    }
    let missing = [
        ("cmake", runtime_dir.join("cmake").is_dir()),
        ("include", runtime_dir.join("include").is_dir()),
        (
            "include/cef_version.h",
            runtime_dir.join("include").join("cef_version.h").is_file(),
        ),
        ("libcef_dll", runtime_dir.join("libcef_dll").is_dir()),
    ]
    .into_iter()
    .filter_map(|(name, ok)| (!ok).then_some(name))
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "CEF runtime at {} is not a complete standard distribution (missing {}). \
Reinstall with `cargo run -p sabine-cli -- runtime install` (or delete that folder and relaunch). \
On Windows, a partial extract often means Git's GNU tar mishandled the path — use the OS `tar`.",
            runtime_dir.display(),
            missing.join(", ")
        ));
    }

    if work_dir.exists() {
        std::fs::remove_dir_all(&work_dir).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(&source_dir).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&build_dir).map_err(|error| error.to_string())?;
    write_host_source(&source_dir)?;

    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(&source_dir)
        .arg("-B")
        .arg(&build_dir);
    apply_cmake_generator(&mut configure)?;
    // Forward slashes so CEF's ADD_LOGICAL_TARGET does not treat \U as an escape.
    let cef_root = runtime_dir.to_string_lossy().replace('\\', "/");
    let output_dir = runtime_dir
        .join(".sabine-hosts")
        .join(expected_stamp)
        .to_string_lossy()
        .replace('\\', "/");
    configure
        .arg("-DCMAKE_BUILD_TYPE=Release")
        .arg(format!("-DCEF_ROOT={cef_root}"))
        .arg(format!("-DSABINE_HOST_OUTPUT_DIR={output_dir}"));
    run_checked(&mut configure)?;
    run_checked(
        Command::new("cmake")
            .arg("--build")
            .arg(&build_dir)
            .arg("--config")
            .arg("Release")
            .arg("--target")
            .arg("sabine-host")
            .arg("--parallel"),
    )?;

    if binary.is_file() {
        Ok(binary)
    } else {
        Err(format!(
            "CEF host build did not create {}",
            binary.display()
        ))
    }
}

pub fn available_host(runtime_dir: &Path) -> Option<PathBuf> {
    prebuilt_host_path().or_else(|| {
        let path = host_release_binary(runtime_dir);
        path.is_file().then_some(path)
    })
}

pub fn smoke_test_runtime(host: &Path, runtime_dir: &Path) -> Result<(), String> {
    let host = host
        .canonicalize()
        .map_err(|error| format!("could not resolve Sabine host {}: {error}", host.display()))?;
    prepare_host_runtime(&host, runtime_dir)?;
    let binary_dir = runtime_binary_directory(runtime_dir);
    let cache_dir = std::env::temp_dir().join(format!(
        "sabine-runtime-probe-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("could not create CEF runtime probe cache: {error}"))?;
    let _cache = TemporaryDirectory(cache_dir.clone());
    let mut command = Command::new(&host);
    configure_background_command(&mut command);
    command
        .arg("--sabine-runtime-smoke-test")
        .arg(format!("--root-cache-path={}", cache_dir.display()))
        .current_dir(&binary_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    apply_runtime_resource_args(&mut command, runtime_dir);
    #[cfg(target_os = "linux")]
    {
        command
            .arg("--headless")
            .arg("--sabine-ozone-platform=headless");
        let release = binary_dir.to_string_lossy();
        let existing = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        command.env(
            "LD_LIBRARY_PATH",
            if existing.is_empty() {
                release.into_owned()
            } else {
                format!("{release}:{existing}")
            },
        );
    }
    #[cfg(target_os = "windows")]
    {
        let release = binary_dir.to_string_lossy();
        let existing = std::env::var("PATH").unwrap_or_default();
        command.env(
            "PATH",
            if existing.is_empty() {
                release.into_owned()
            } else {
                format!("{release};{existing}")
            },
        );
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start CEF runtime probe: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not wait for CEF runtime probe: {error}"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("CEF runtime probe timed out after 30 seconds".to_string());
        }
        thread::sleep(Duration::from_millis(50));
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if status.success() {
        Ok(())
    } else {
        let details = if stderr.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", stderr.trim())
        };
        Err(format!("CEF runtime probe exited with {status}{details}"))
    }
}

pub(crate) fn configure_background_command(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = command;
}

pub fn runtime_binary_directory(runtime_dir: &Path) -> PathBuf {
    let release = runtime_dir.join("Release");
    if release.is_dir() {
        release
    } else {
        runtime_dir.to_path_buf()
    }
}

pub fn prepare_host_runtime(host: &Path, runtime_dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let framework = [runtime_dir.join("Release"), runtime_dir.to_path_buf()]
            .into_iter()
            .map(|root| root.join("Chromium Embedded Framework.framework"))
            .find(|path| path.is_dir())
            .ok_or_else(|| {
                format!(
                    "CEF runtime at {} has no Chromium framework",
                    runtime_dir.display()
                )
            })?;
        let app = host
            .ancestors()
            .find(|path| path.extension().is_some_and(|extension| extension == "app"))
            .ok_or_else(|| {
                format!(
                    "macOS Sabine host is not inside an app bundle: {}",
                    host.display()
                )
            })?;
        let destination = app
            .join("Contents/Frameworks")
            .join("Chromium Embedded Framework.framework");
        match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let current = std::fs::read_link(&destination).ok().map(|path| {
                    if path.is_absolute() {
                        path
                    } else {
                        destination
                            .parent()
                            .expect("framework path has a parent")
                            .join(path)
                    }
                });
                if current.as_deref().and_then(|path| path.canonicalize().ok())
                    == framework.canonicalize().ok()
                {
                    return Ok(());
                }
                std::fs::remove_file(&destination).map_err(|error| {
                    format!("could not replace stale CEF framework link: {error}")
                })?;
            }
            Ok(metadata) if metadata.is_dir() => return Ok(()),
            Ok(_) => {
                std::fs::remove_file(&destination).map_err(|error| {
                    format!("could not replace invalid CEF framework entry: {error}")
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("could not inspect Sabine host framework: {error}"));
            }
        }
        let parent = destination.parent().expect("framework path has a parent");
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("could not prepare Sabine host framework directory: {error}")
        })?;
        std::os::unix::fs::symlink(&framework, &destination)
            .map_err(|error| format!("could not link Sabine host to the CEF runtime: {error}"))?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (host, runtime_dir);
    Ok(())
}

struct TemporaryDirectory(PathBuf);

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn prebuilt_host_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SABINE_HOST_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let path = installed_host_path(directory);
        if path.is_file() {
            return Some(path);
        }
    }
    if cfg!(debug_assertions) {
        return None;
    }
    let root = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Sabine")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?)
            .join("Library/Application Support")
            .join("Sabine")
    } else if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(path).join("sabine")
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".local/share/sabine")
    };
    #[derive(Deserialize)]
    struct CurrentSystem {
        active: String,
    }
    let bin = root.join("bin");
    let current =
        serde_json::from_slice::<CurrentSystem>(&std::fs::read(bin.join("current.json")).ok()?)
            .ok()?;
    let package_version = env!("CARGO_PKG_VERSION");
    let public_version = package_version
        .strip_suffix(".0")
        .unwrap_or(package_version);
    if current.active != public_version {
        return None;
    }
    let directory = bin.join("versions").join(current.active);
    let path = installed_host_path(&directory);
    path.is_file().then_some(path)
}

fn installed_host_path(directory: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        directory.join("sabine-host.app/Contents/MacOS/sabine-host")
    } else {
        directory.join(host_binary_name())
    }
}

pub fn apply_runtime_resource_args(command: &mut Command, runtime_dir: &Path) {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let resources = runtime_dir.join("Resources");
        command
            .arg(format!(
                "--sabine-resources-dir-path={}",
                resources.display()
            ))
            .arg(format!(
                "--sabine-locales-dir-path={}",
                resources.join("locales").display()
            ));
    }
    #[cfg(target_os = "macos")]
    {
        let framework_parent = [runtime_dir.join("Release"), runtime_dir.to_path_buf()]
            .into_iter()
            .find(|root| root.join("Chromium Embedded Framework.framework").is_dir())
            .unwrap_or_else(|| runtime_dir.join("Release"));
        let framework = framework_parent.join("Chromium Embedded Framework.framework");
        let resources = framework.join("Resources");
        let existing = std::env::var("DYLD_FRAMEWORK_PATH").unwrap_or_default();
        command
            .arg(format!(
                "--sabine-framework-dir-path={}",
                framework.display()
            ))
            .arg(format!(
                "--sabine-resources-dir-path={}",
                resources.display()
            ))
            .env(
                "DYLD_FRAMEWORK_PATH",
                if existing.is_empty() {
                    framework_parent.display().to_string()
                } else {
                    format!("{}:{existing}", framework_parent.display())
                },
            );
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    let _ = (command, runtime_dir);
}

pub fn host_source_fingerprint() -> String {
    sources::host_source_fingerprint()
}

pub fn runtime_probe_fingerprint() -> String {
    format!("{}-{RUNTIME_PROBE_VERSION}", host_source_fingerprint())
}

fn run_checked(command: &mut Command) -> Result<(), String> {
    configure_background_command(command);
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "command failed: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

#[cfg(test)]
mod tests {
    use super::{runtime_binary_directory, sources::HOST_SOURCES};

    #[test]
    fn runtime_binary_directory_accepts_flat_platform_layouts() {
        let root =
            std::env::temp_dir().join(format!("sabine-host-runtime-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(runtime_binary_directory(&root), root);
        std::fs::create_dir_all(root.join("Release")).unwrap();
        assert_eq!(runtime_binary_directory(&root), root.join("Release"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn host_sources_match_cmake_lists() {
        let cmake = HOST_SOURCES
            .iter()
            .find(|(name, _)| *name == "CMakeLists.txt")
            .map(|(_, body)| *body)
            .expect("CMakeLists.txt embedded");
        let start = cmake
            .find("set(SABINE_HOST_SOURCES")
            .expect("SABINE_HOST_SOURCES");
        let list = &cmake[start..];
        let end = list.find(')').expect("closing paren");
        let listed: Vec<&str> = list[..end]
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let embedded: Vec<&str> = HOST_SOURCES
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| {
                !matches!(
                    *name,
                    "CMakeLists.txt" | "main_mac.mm" | "mac/Info.plist.in"
                )
            })
            .collect();
        assert_eq!(
            listed, embedded,
            "HOST_SOURCES must list every CMake host source in the same order"
        );
        assert!(cmake.contains("list(APPEND SABINE_HOST_SOURCES main_mac.mm)"));
        assert!(cmake.contains("mac/Info.plist.in"));
    }
}
