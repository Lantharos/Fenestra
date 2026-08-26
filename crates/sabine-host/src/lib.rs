//! CEF host embedder sources and cmake build.
//!
//! Apps and the CLI call [`ensure_host`] to materialize `sabine-host` next to
//! a CEF runtime. Process launch and window wiring stay in the `sabine` crate.

use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use sabine_bridge::INSTALL_SCRIPT;
use serde::Deserialize;

const HOST_SOURCES: &[(&str, &str)] = &[
    ("CMakeLists.txt", include_str!("../shared/CMakeLists.txt")),
    ("main.cc", include_str!("../shared/main.cc")),
    ("entry.h", include_str!("../shared/entry.h")),
    ("main_mac.mm", include_str!("../shared/main_mac.mm")),
    (
        "mac/Info.plist.in",
        include_str!("../shared/mac/Info.plist.in"),
    ),
    ("app/app.cc", include_str!("../shared/app/app.cc")),
    ("app/app.h", include_str!("../shared/app/app.h")),
    ("common/json.cc", include_str!("../shared/common/json.cc")),
    ("common/json.h", include_str!("../shared/common/json.h")),
    ("guest/input.cc", include_str!("../shared/guest/input.cc")),
    ("guest/input.h", include_str!("../shared/guest/input.h")),
    (
        "guest/manager.cc",
        include_str!("../shared/guest/manager.cc"),
    ),
    ("guest/manager.h", include_str!("../shared/guest/manager.h")),
    ("osr/handler.cc", include_str!("../shared/osr/handler.cc")),
    ("osr/handler.h", include_str!("../shared/osr/handler.h")),
    ("osr/bridge.cc", include_str!("../shared/osr/bridge.cc")),
    (
        "osr/callbacks.cc",
        include_str!("../shared/osr/callbacks.cc"),
    ),
    (
        "osr/downloads.cc",
        include_str!("../shared/osr/downloads.cc"),
    ),
    ("osr/drag.cc", include_str!("../shared/osr/drag.cc")),
    (
        "osr/guest/commands.cc",
        include_str!("../shared/osr/guest/commands.cc"),
    ),
    (
        "osr/guest/lifecycle.cc",
        include_str!("../shared/osr/guest/lifecycle.cc"),
    ),
    ("osr/ime.cc", include_str!("../shared/osr/ime.cc")),
    ("osr/ime.h", include_str!("../shared/osr/ime.h")),
    ("osr/input.cc", include_str!("../shared/osr/input.cc")),
    ("osr/screen.cc", include_str!("../shared/osr/screen.cc")),
    ("osr/screen.h", include_str!("../shared/osr/screen.h")),
    ("osr/tasks.cc", include_str!("../shared/osr/tasks.cc")),
    ("osr/tasks.h", include_str!("../shared/osr/tasks.h")),
    (
        "osr/transport.cc",
        include_str!("../shared/osr/transport.cc"),
    ),
    (
        "osr/utilities.cc",
        include_str!("../shared/osr/utilities.cc"),
    ),
    ("osr/utilities.h", include_str!("../shared/osr/utilities.h")),
    (
        "osr/accelerated/paint.cc",
        include_str!("../shared/osr/accelerated/paint.cc"),
    ),
    (
        "osr/accelerated/paint.h",
        include_str!("../shared/osr/accelerated/paint.h"),
    ),
    (
        "osr/accelerated/protocol.cc",
        include_str!("../shared/osr/accelerated/protocol.cc"),
    ),
    (
        "osr/accelerated/protocol.h",
        include_str!("../shared/osr/accelerated/protocol.h"),
    ),
    (
        "osr/accelerated/windows/d3d11_copy.cc",
        include_str!("../shared/osr/accelerated/windows/d3d11_copy.cc"),
    ),
    (
        "osr/accelerated/windows/d3d11_copy.h",
        include_str!("../shared/osr/accelerated/windows/d3d11_copy.h"),
    ),
];

const HOST_BUILD_LOCK_TIMEOUT: Duration = Duration::from_secs(600);
const HOST_BUILD_LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);
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
    prepare_host_runtime(host, runtime_dir)?;
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
    let mut command = Command::new(host);
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
    if current.active != env!("CARGO_PKG_VERSION") {
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

fn apply_cmake_generator(configure: &mut Command) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        // Official Windows CEF is MSVC-only; MSYS/MinGW often wins CMake's default.
        let generator = windows_msvc_generator()?;
        configure.arg("-G").arg(generator).arg("-A").arg("x64");
        return Ok(());
    }
    if command_available("ninja") {
        configure.arg("-G").arg("Ninja");
    } else {
        configure.arg("-G").arg("Unix Makefiles");
    }
    Ok(())
}

fn windows_msvc_generator() -> Result<&'static str, String> {
    if let Some(version) = vswhere_installation_version() {
        if version.starts_with("18.") {
            return Ok("Visual Studio 18 2026");
        }
        if version.starts_with("17.") {
            return Ok("Visual Studio 17 2022");
        }
        if version.starts_with("16.") {
            return Ok("Visual Studio 16 2019");
        }
    }
    for generator in [
        "Visual Studio 17 2022",
        "Visual Studio 16 2019",
        "Visual Studio 18 2026",
    ] {
        if cmake_generator_available(generator) {
            return Ok(generator);
        }
    }
    Err(
        "building sabine-host on Windows requires Visual Studio 2019+ with the C++ workload (MSVC). \
MSYS/MinGW cannot link the official CEF runtime."
            .into(),
    )
}

fn vswhere_installation_version() -> Option<String> {
    let vswhere =
        PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    if !vswhere.is_file() {
        return None;
    }
    let output = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationVersion",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!version.is_empty()).then_some(version)
}

fn cmake_generator_available(generator: &str) -> bool {
    Command::new("cmake")
        .args(["-G", generator, "-A", "x64", "--help"])
        .output()
        .ok()
        .is_some_and(|output| {
            let help = String::from_utf8_lossy(&output.stdout);
            help.lines().any(|line| line.contains(generator))
        })
}

fn write_host_source(source_dir: &Path) -> Result<(), String> {
    for (name, body) in HOST_SOURCES {
        let path = source_dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, body).map_err(|error| error.to_string())?;
    }
    std::fs::write(source_dir.join("sabine_bridge_js.h"), bridge_js_header())
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn bridge_js_header() -> String {
    let mut output = String::new();
    output.push_str(
        "// AUTO-GENERATED by sabine-host from\n\
         // crates/sabine-bridge/src/web_bridge.js. Do not edit by hand.\n\
         #pragma once\n\
         constexpr const char* SABINE_BRIDGE_JS_RAW = R\"js(",
    );
    for byte in INSTALL_SCRIPT.as_bytes() {
        output.push(*byte as char);
    }
    output.push_str(")js\";\n");
    output
}

pub fn host_source_fingerprint() -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for (name, body) in HOST_SOURCES {
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
        for byte in body.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    for byte in INSTALL_SCRIPT.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn runtime_probe_fingerprint() -> String {
    format!("{}-{RUNTIME_PROBE_VERSION}", host_source_fingerprint())
}

fn command_available(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn run_checked(command: &mut Command) -> Result<(), String> {
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

struct HostBuildLock {
    path: PathBuf,
}

impl HostBuildLock {
    fn acquire(runtime_dir: &Path) -> Result<Self, String> {
        let path = runtime_dir.join(".sabine-host-build.lock");
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    let _ = writeln!(file, "started={}", unix_timestamp_secs());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= HOST_BUILD_LOCK_TIMEOUT {
                        return Err(format!(
                            "timed out waiting for Sabine CEF host build lock at {}",
                            path.display()
                        ));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

impl Drop for HostBuildLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path) -> bool {
    if let Some(pid) = lock_holder_pid(path) {
        return !process_alive(pid);
    }
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= HOST_BUILD_LOCK_STALE_AFTER)
}

fn lock_holder_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("pid=")
                .and_then(|value| value.trim().parse().ok())
        })
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|output| output.lines().next().map(str::to_string))
            .and_then(|line| line.split(',').nth(1).map(str::to_string))
            .is_some_and(|value| value.trim_matches('"') == pid.to_string())
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{HOST_SOURCES, runtime_binary_directory};

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
