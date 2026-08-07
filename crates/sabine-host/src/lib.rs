//! CEF host embedder sources and cmake build.
//!
//! Apps and the CLI call [`ensure_host`] to materialize `sabine-host` next to
//! a CEF runtime. Process launch and window wiring stay in the `sabine` crate.

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use sabine_bridge::INSTALL_SCRIPT;

const HOST_SOURCES: &[(&str, &str)] = &[
    ("CMakeLists.txt", include_str!("../shared/CMakeLists.txt")),
    ("main.cc", include_str!("../shared/main.cc")),
    ("app.cc", include_str!("../shared/app.cc")),
    ("app.h", include_str!("../shared/app.h")),
    ("guest_input.cc", include_str!("../shared/guest_input.cc")),
    ("guest_input.h", include_str!("../shared/guest_input.h")),
    (
        "guest_manager.cc",
        include_str!("../shared/guest_manager.cc"),
    ),
    ("guest_manager.h", include_str!("../shared/guest_manager.h")),
    ("json_util.cc", include_str!("../shared/json_util.cc")),
    ("json_util.h", include_str!("../shared/json_util.h")),
    ("osr_handler.cc", include_str!("../shared/osr_handler.cc")),
    ("osr_handler.h", include_str!("../shared/osr_handler.h")),
    (
        "osr_handler_accel_ipc.cc",
        include_str!("../shared/osr_handler_accel_ipc.cc"),
    ),
    (
        "osr_handler_accel_ipc.h",
        include_str!("../shared/osr_handler_accel_ipc.h"),
    ),
    (
        "osr_handler_accelerated.cc",
        include_str!("../shared/osr_handler_accelerated.cc"),
    ),
    (
        "osr_handler_accelerated.h",
        include_str!("../shared/osr_handler_accelerated.h"),
    ),
    (
        "osr_handler_bridge.cc",
        include_str!("../shared/osr_handler_bridge.cc"),
    ),
    (
        "osr_handler_cef.cc",
        include_str!("../shared/osr_handler_cef.cc"),
    ),
    (
        "osr_handler_downloads.cc",
        include_str!("../shared/osr_handler_downloads.cc"),
    ),
    (
        "osr_handler_drag.cc",
        include_str!("../shared/osr_handler_drag.cc"),
    ),
    (
        "osr_handler_guest.cc",
        include_str!("../shared/osr_handler_guest.cc"),
    ),
    (
        "osr_handler_guest_ops.cc",
        include_str!("../shared/osr_handler_guest_ops.cc"),
    ),
    (
        "osr_handler_ime.cc",
        include_str!("../shared/osr_handler_ime.cc"),
    ),
    (
        "osr_handler_ime.h",
        include_str!("../shared/osr_handler_ime.h"),
    ),
    (
        "osr_handler_input.cc",
        include_str!("../shared/osr_handler_input.cc"),
    ),
    (
        "osr_handler_ipc.cc",
        include_str!("../shared/osr_handler_ipc.cc"),
    ),
    (
        "osr_handler_screen.cc",
        include_str!("../shared/osr_handler_screen.cc"),
    ),
    (
        "osr_handler_screen.h",
        include_str!("../shared/osr_handler_screen.h"),
    ),
    (
        "osr_handler_util.cc",
        include_str!("../shared/osr_handler_util.cc"),
    ),
    (
        "osr_handler_util.h",
        include_str!("../shared/osr_handler_util.h"),
    ),
];

const HOST_BUILD_LOCK_TIMEOUT: Duration = Duration::from_secs(600);
const HOST_BUILD_LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

pub fn host_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sabine-host.exe"
    } else {
        "sabine-host"
    }
}

pub fn host_release_binary(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("Release").join(host_binary_name())
}

pub fn ensure_host(runtime_dir: &Path) -> Result<PathBuf, String> {
    let binary = host_release_binary(runtime_dir);
    let source_dir = runtime_dir.join(".sabine-host-src");
    let build_dir = runtime_dir.join(".sabine-host-build");
    let source_stamp = build_dir.join("sabine-host-source.fnv");
    let expected_stamp = host_source_fingerprint();
    if binary.is_file()
        && std::fs::read_to_string(&source_stamp).is_ok_and(|stamp| stamp.trim() == expected_stamp)
    {
        return Ok(binary);
    }
    let _lock = HostBuildLock::acquire(runtime_dir)?;
    if binary.is_file()
        && std::fs::read_to_string(&source_stamp).is_ok_and(|stamp| stamp.trim() == expected_stamp)
    {
        return Ok(binary);
    }
    if !runtime_dir.join("include").is_dir()
        || !runtime_dir.join("libcef_dll").is_dir()
        || !runtime_dir.join("cmake").is_dir()
    {
        return Err(format!(
            "CEF runtime at {} is not a standard CEF distribution",
            runtime_dir.display()
        ));
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
    configure
        .arg("-DCMAKE_BUILD_TYPE=Release")
        .arg(format!("-DCEF_ROOT={cef_root}"));
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
        std::fs::write(source_stamp, expected_stamp).map_err(|error| error.to_string())?;
        Ok(binary)
    } else {
        Err(format!(
            "CEF host build did not create {}",
            binary.display()
        ))
    }
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
        std::fs::write(source_dir.join(name), body).map_err(|error| error.to_string())?;
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

fn host_source_fingerprint() -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for (_, body) in HOST_SOURCES {
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
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= HOST_BUILD_LOCK_STALE_AFTER)
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::HOST_SOURCES;

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
            .filter(|name| *name != "CMakeLists.txt")
            .collect();
        assert_eq!(
            listed, embedded,
            "HOST_SOURCES must list every CMake host source in the same order"
        );
    }
}
