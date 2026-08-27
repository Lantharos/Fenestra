use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::registry::replace_file;
use crate::types::{AppArtifact, AppArtifactKind, PendingAppUpdate, ServiceError, ServiceResult};
use sabine_runtime::background_command;

pub(super) fn install_archive(
    root: &Path,
    id: &str,
    version: &str,
    artifact: &AppArtifact,
    release_dir: &Path,
) -> ServiceResult<()> {
    let downloads = root.join("downloads").join(id);
    std::fs::create_dir_all(&downloads)?;
    let archive = downloads.join(format!("{version}.archive"));
    download_artifact(&artifact.url, &archive)?;
    verify_sha256(&archive, &artifact.sha256)?;
    let staging = release_dir.with_extension("installing");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    extract_archive(&archive, &staging, &artifact.url)?;
    if let Some(parent) = release_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(staging, release_dir)?;
    let _ = std::fs::remove_file(archive);
    Ok(())
}

pub(super) fn download_artifact(url: &str, destination: &Path) -> ServiceResult<()> {
    let temporary = destination.with_extension("download");
    let response = ureq::get(url)
        .call()
        .map_err(|error| ServiceError::Update(format!("artifact download failed: {error}")))?;
    let (_, body) = response.into_parts();
    let mut reader = body.into_reader();
    let mut file = File::create(&temporary)?;
    std::io::copy(&mut reader, &mut file)?;
    file.flush()?;
    file.sync_all()?;
    replace_file(&temporary, destination)?;
    Ok(())
}

pub(super) fn verify_sha256(path: &Path, expected: &str) -> ServiceResult<()> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(ServiceError::Update(
            "artifact SHA-256 mismatch".to_string(),
        ))
    }
}

pub(super) fn extract_archive(archive: &Path, destination: &Path, url: &str) -> ServiceResult<()> {
    let zip = url.to_ascii_lowercase().ends_with(".zip");
    let (program, list_args, extract_args): (&str, &[&str], &[&str]) = if zip {
        ("unzip", &["-Z1"], &["-q"])
    } else {
        ("tar", &["-tf"], &["-xf"])
    };
    let listing = background_command(program)
        .args(list_args)
        .arg(archive)
        .output()
        .map_err(|error| ServiceError::Update(format!("failed to inspect archive: {error}")))?;
    if !listing.status.success() {
        return Err(ServiceError::Update(
            "could not inspect update archive".to_string(),
        ));
    }
    for entry in String::from_utf8_lossy(&listing.stdout).lines() {
        if !safe_relative_path(Path::new(entry)) {
            return Err(ServiceError::Update(
                "update archive contains an unsafe path".to_string(),
            ));
        }
    }
    let mut command = background_command(program);
    command.args(extract_args).arg(archive);
    if zip {
        command.arg("-d").arg(destination);
    } else {
        command.arg("-C").arg(destination);
    }
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| ServiceError::Update(format!("failed to extract archive: {error}")))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| ServiceError::Update("update archive extraction failed".to_string()))
}

pub(super) fn run_package_installer(
    update: &PendingAppUpdate,
    install_target: Option<&Path>,
) -> ServiceResult<()> {
    let mut command = match update.kind {
        AppArtifactKind::Deb => {
            elevated_command("apt-get", &["install", "--yes"], &update.artifact)?
        }
        AppArtifactKind::Rpm => {
            if command_exists("dnf") {
                elevated_command("dnf", &["install", "--assumeyes"], &update.artifact)?
            } else {
                elevated_command("rpm", &["-U"], &update.artifact)?
            }
        }
        AppArtifactKind::Msi => {
            let mut command = Command::new("msiexec");
            command
                .arg("/i")
                .arg(&update.artifact)
                .args(["/passive", "/norestart"]);
            command
        }
        AppArtifactKind::Exe => Command::new(&update.artifact),
        AppArtifactKind::Dmg => return install_dmg(update, install_target),
        AppArtifactKind::AppImage => return install_appimage(update, install_target),
        AppArtifactKind::Archive => {
            return Err(ServiceError::Update(
                "archive updates do not use a package installer".to_string(),
            ));
        }
    };
    let status = command
        .stdin(Stdio::null())
        .status()
        .map_err(|error| ServiceError::Update(format!("failed to start installer: {error}")))?;
    let success = status.success()
        || (matches!(update.kind, AppArtifactKind::Msi)
            && status.code().is_some_and(|code| code == 3010));
    success
        .then_some(())
        .ok_or_else(|| ServiceError::Update(format!("installer exited with {status}")))
}

pub(super) fn install_appimage(
    update: &PendingAppUpdate,
    target: Option<&Path>,
) -> ServiceResult<()> {
    if !cfg!(target_os = "linux") {
        return Err(ServiceError::Update(
            "AppImage updates are only supported on Linux".to_string(),
        ));
    }
    let target = target.ok_or_else(|| {
        ServiceError::Update("AppImage update is missing its installation path".to_string())
    })?;
    let temporary = target.with_extension("sabine-update");
    std::fs::copy(&update.artifact, &temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&temporary)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&temporary, permissions)?;
    }
    replace_file(&temporary, target)?;
    Ok(())
}

pub(super) fn install_dmg(update: &PendingAppUpdate, target: Option<&Path>) -> ServiceResult<()> {
    if !cfg!(target_os = "macos") {
        return Err(ServiceError::Update(
            "DMG updates are only supported on macOS".to_string(),
        ));
    }
    let executable = target.ok_or_else(|| {
        ServiceError::Update("DMG update is missing its application path".to_string())
    })?;
    let app = executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .ok_or_else(|| ServiceError::Update("could not locate installed macOS app".to_string()))?;
    let mount = update.artifact.with_extension("mount");
    if mount.exists() {
        let _ = std::fs::remove_dir_all(&mount);
    }
    std::fs::create_dir_all(&mount)?;
    let attach = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount)
        .arg(&update.artifact)
        .status()?;
    if !attach.success() {
        return Err(ServiceError::Update(
            "could not mount app update".to_string(),
        ));
    }
    let result = (|| {
        let source = std::fs::read_dir(&mount)?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "app"))
            .ok_or_else(|| {
                ServiceError::Update("DMG contains no application bundle".to_string())
            })?;
        let source = shell_single_quote(&source.display().to_string());
        let destination = shell_single_quote(&app.display().to_string());
        let staged = shell_single_quote(&format!("{}.sabine-new", app.display()));
        let backup = shell_single_quote(&format!("{}.sabine-old", app.display()));
        let script = format!(
            "/bin/rm -rf {staged} {backup} && /usr/bin/ditto {source} {staged} && /bin/mv {destination} {backup} && (/bin/mv {staged} {destination} && /bin/rm -rf {backup} || (/bin/mv {backup} {destination}; exit 1))"
        );
        let status = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "do shell script {} with administrator privileges",
                apple_script_string(&script)
            ))
            .status()?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| ServiceError::Update("macOS app installation was cancelled".to_string()))
    })();
    let _ = Command::new("hdiutil").arg("detach").arg(&mount).status();
    let _ = std::fs::remove_dir_all(mount);
    result
}

pub(super) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn apple_script_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(super) fn elevated_command(
    program: &str,
    args: &[&str],
    artifact: &Path,
) -> ServiceResult<Command> {
    if !cfg!(target_os = "linux") {
        return Err(ServiceError::Update(
            "this package installer is unsupported on the current platform".to_string(),
        ));
    }
    let mut command = Command::new("pkexec");
    command.arg(program).args(args).arg(artifact);
    Ok(command)
}

pub(super) fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

pub(super) fn artifact_extension(kind: AppArtifactKind) -> &'static str {
    match kind {
        AppArtifactKind::Archive => "archive",
        AppArtifactKind::Deb => "deb",
        AppArtifactKind::Rpm => "rpm",
        AppArtifactKind::Msi => "msi",
        AppArtifactKind::Exe => "exe",
        AppArtifactKind::Dmg => "dmg",
        AppArtifactKind::AppImage => "AppImage",
    }
}

pub(super) fn pending_path(root: &Path, id: &str) -> PathBuf {
    root.join("apps").join(id).join("pending-update.json")
}

pub(super) fn write_json_atomic(path: &Path, value: &impl serde::Serialize) -> ServiceResult<()> {
    let temporary = path.with_extension("new");
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value).expect("pending update is serializable"))?;
    file.sync_all()?;
    replace_file(&temporary, path)?;
    Ok(())
}

pub(crate) fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}
