use std::{path::PathBuf, process::Command};

use crate::sources::command_available;

pub(crate) fn apply_cmake_generator(configure: &mut Command) -> Result<(), String> {
    if cfg!(target_os = "windows") {
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
    let mut command = Command::new(vswhere);
    crate::configure_background_command(&mut command);
    let output = command
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
    let mut command = Command::new("cmake");
    crate::configure_background_command(&mut command);
    command
        .args(["-G", generator, "-A", "x64", "--help"])
        .output()
        .ok()
        .is_some_and(|output| {
            let help = String::from_utf8_lossy(&output.stdout);
            help.lines().any(|line| line.contains(generator))
        })
}
