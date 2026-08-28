use base64::{Engine, engine::general_purpose::STANDARD};
use sabine_service::{
    AppArtifact, AppArtifactKind, AppReleaseManifest, AppUpdateSource, SystemReleaseArtifact,
    SystemReleaseManifest, UpdatePolicy, public_key_from_private, sign_app_release,
    sign_system_release,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Deserialize)]
struct ReleaseConfig {
    app: AppConfig,
    updates: Option<sabine_service::AppUpdateConfig>,
}

#[derive(Deserialize)]
struct AppConfig {
    id: String,
    version: String,
}

pub(crate) fn write_manifest(
    source: &Path,
    output: &Path,
    channel: &str,
    artifact_specs: &[String],
    executable_specs: &[String],
) -> Result<(), String> {
    let source = absolute(source)?;
    let config_path = source.join("Sabine.toml");
    let config = toml::from_str::<ReleaseConfig>(
        &std::fs::read_to_string(&config_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("failed to parse {}: {error}", config_path.display()))?;
    let repository = match config.updates.as_ref() {
        Some(updates) if updates.policy == UpdatePolicy::Disabled => {
            return Err("updates are disabled in Sabine.toml".to_string());
        }
        Some(updates) => match &updates.source {
            AppUpdateSource::Github { repository } => repository.clone(),
            AppUpdateSource::Http { .. } => {
                return Err(
                    "release manifest generation currently requires the GitHub provider".into(),
                );
            }
        },
        None => std::env::var("GITHUB_REPOSITORY")
            .map_err(|_| "configure [updates] or run inside GitHub Actions".to_string())?,
    };
    let private_key = signing_key()?;
    let signer_public_key =
        public_key_from_private(&private_key).map_err(|error| error.to_string())?;
    if let Some(configured) = config
        .updates
        .as_ref()
        .map(|updates| updates.public_key.trim())
        .filter(|key| !key.is_empty())
        && configured != signer_public_key
    {
        return Err("Sabine.toml update public_key does not match the release signing key".into());
    }
    let tag = format!("v{}", config.app.version);
    if let Ok(ref_name) = std::env::var("GITHUB_REF_NAME")
        && ref_name != tag
    {
        return Err(format!(
            "release tag {ref_name} does not match Sabine.toml version {tag}"
        ));
    }
    if let Some(updates) = &config.updates
        && updates.channel != channel
    {
        return Err(format!(
            "requested channel {channel} does not match Sabine.toml channel {}",
            updates.channel
        ));
    }
    let executables = parse_assignments(executable_specs)?;
    let mut artifacts = BTreeMap::new();
    for (target, raw_path) in parse_assignments(artifact_specs)? {
        let path = absolute_from(&source, Path::new(&raw_path));
        if !path.is_file() {
            return Err(format!(
                "release artifact does not exist: {}",
                path.display()
            ));
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("invalid artifact name: {}", path.display()))?;
        let kind = artifact_kind(&path)?;
        artifacts.insert(
            target.clone(),
            AppArtifact {
                url: format!("https://github.com/{repository}/releases/download/{tag}/{file_name}"),
                sha256: sha256(&path)?,
                kind,
                executable: executables.get(&target).map(PathBuf::from),
            },
        );
    }
    if artifacts.is_empty() {
        return Err("at least one --artifact target=path is required".to_string());
    }
    let published_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());
    let mut manifest = AppReleaseManifest {
        schema: 1,
        app_id: config.app.id,
        version: config.app.version,
        channel: channel.to_string(),
        published_at,
        requires_sabine: sabine_service::SabineVersion::current(),
        artifacts,
        signature: String::new(),
    };
    sign_app_release(&mut manifest, &private_key).map_err(|error| error.to_string())?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn write_system_manifest(
    version: &str,
    directory: &Path,
    output: &Path,
) -> Result<(), String> {
    let version = version.trim_start_matches('v');
    if version.is_empty() {
        return Err("system release version is required".to_string());
    }
    if sabine_service::SabineVersion::parse(version)
        != Some(sabine_service::SabineVersion::current())
    {
        return Err(format!(
            "system release version must be {}",
            sabine_service::SABINE_VERSION
        ));
    }
    let directory = absolute(directory)?;
    let mut artifacts = BTreeMap::new();
    for entry in std::fs::read_dir(&directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if !path.is_file()
            || path
                .file_name()
                .is_some_and(|name| name == "sabine-release.json")
        {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("invalid artifact name: {}", path.display()))?
            .to_string();
        artifacts.insert(
            name.clone(),
            SystemReleaseArtifact {
                sha256: sha256(&path)?,
                size: std::fs::metadata(&path)
                    .map_err(|error| error.to_string())?
                    .len(),
                url: format!(
                    "https://github.com/Lantharos/Sabine/releases/download/v{version}/{name}"
                ),
            },
        );
    }
    if artifacts.is_empty() {
        return Err("system release directory contains no artifacts".to_string());
    }
    let published_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());
    let mut manifest = SystemReleaseManifest {
        schema: 1,
        version: version.to_string(),
        published_at,
        compatibility: sabine_service::SystemCompatibility::current(),
        artifacts,
        signature: String::new(),
    };
    sign_system_release(&mut manifest, &signing_key()?).map_err(|error| error.to_string())?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn generate_signing_key(public_output: &Path) -> Result<String, String> {
    let private_key = new_private_key()?;
    let public_key = public_key_from_private(&private_key).map_err(|error| error.to_string())?;
    if let Some(parent) = public_output.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(public_output, format!("{public_key}\n")).map_err(|error| error.to_string())?;
    Ok(private_key)
}

pub(crate) fn initialize_github_release(repository: Option<&str>) -> Result<String, String> {
    let repository = repository
        .map(str::to_string)
        .or_else(|| std::env::var("GITHUB_REPOSITORY").ok())
        .or_else(|| {
            Command::new("gh")
                .args([
                    "repo",
                    "view",
                    "--json",
                    "nameWithOwner",
                    "--jq",
                    ".nameWithOwner",
                ])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        })
        .filter(|repository| repository.split_once('/').is_some())
        .ok_or_else(|| {
            "could not determine GitHub repository; pass --repository owner/name".to_string()
        })?;
    let private_key = new_private_key()?;
    let mut child = Command::new("gh")
        .args([
            "secret",
            "set",
            "SABINE_UPDATE_SIGNING_KEY",
            "--repo",
            &repository,
        ])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start GitHub CLI: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "GitHub CLI did not open secret input".to_string())?
        .write_all(private_key.as_bytes())
        .map_err(|error| error.to_string())?;
    let status = child.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("could not configure the app update signing secret".to_string());
    }
    let status = Command::new("gh")
        .args([
            "api",
            "--method",
            "PUT",
            "-H",
            "X-GitHub-Api-Version: 2026-03-10",
            &format!("repos/{repository}/immutable-releases"),
        ])
        .status()
        .map_err(|error| format!("failed to enable immutable releases: {error}"))?;
    if !status.success() {
        return Err("could not enable immutable GitHub Releases".to_string());
    }
    let public_key = public_key_from_private(&private_key).map_err(|error| error.to_string())?;
    configure_app_updates(&repository, &public_key)?;
    Ok(public_key)
}

fn configure_app_updates(repository: &str, public_key: &str) -> Result<(), String> {
    let path = std::env::current_dir()
        .map_err(|error| error.to_string())?
        .join("Sabine.toml");
    if !path.is_file() {
        return Ok(());
    }
    let source = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let mut document = source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    document["updates"]["provider"] = toml_edit::value("github");
    document["updates"]["repository"] = toml_edit::value(repository);
    document["updates"]["channel"] = toml_edit::value("stable");
    document["updates"]["policy"] = toml_edit::value("automatic");
    document["updates"]["public_key"] = toml_edit::value(public_key);
    std::fs::write(path, document.to_string()).map_err(|error| error.to_string())
}

pub(crate) fn signing_public_key() -> Result<String, String> {
    public_key_from_private(&signing_key()?).map_err(|error| error.to_string())
}

fn signing_key() -> Result<String, String> {
    std::env::var("SABINE_UPDATE_SIGNING_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| "SABINE_UPDATE_SIGNING_KEY is required".to_string())
}

fn new_private_key() -> Result<String, String> {
    let mut seed = [0_u8; 32];
    getrandom::fill(&mut seed).map_err(|error| error.to_string())?;
    Ok(STANDARD.encode(seed))
}

fn parse_assignments(values: &[String]) -> Result<BTreeMap<String, String>, String> {
    values
        .iter()
        .map(|value| {
            let (key, value) = value
                .split_once('=')
                .ok_or_else(|| format!("expected target=value, got `{value}`"))?;
            if key.is_empty() || value.is_empty() {
                return Err(format!("expected target=value, got `{value}`"));
            }
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

fn artifact_kind(path: &Path) -> Result<AppArtifactKind, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tar.zst") || name.ends_with(".zip") {
        Ok(AppArtifactKind::Archive)
    } else if name.ends_with(".deb") {
        Ok(AppArtifactKind::Deb)
    } else if name.ends_with(".rpm") {
        Ok(AppArtifactKind::Rpm)
    } else if name.ends_with(".msi") {
        Ok(AppArtifactKind::Msi)
    } else if name.ends_with(".exe") {
        Ok(AppArtifactKind::Exe)
    } else if name.ends_with(".dmg") {
        Ok(AppArtifactKind::Dmg)
    } else if name.ends_with(".appimage") {
        Ok(AppArtifactKind::AppImage)
    } else {
        Err(format!(
            "cannot infer update artifact kind from {}",
            path.display()
        ))
    }
}

fn sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    path.canonicalize().map_err(|error| error.to_string())
}

fn absolute_from(source: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        source.join(path)
    }
}
