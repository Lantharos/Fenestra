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

    if source_dir.exists() {
        std::fs::remove_dir_all(&source_dir).map_err(|error| error.to_string())?;
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

fn host_source_fingerprint() -> String {
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
