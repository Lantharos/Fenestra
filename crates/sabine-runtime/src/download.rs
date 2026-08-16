use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::error::RuntimeError;
use crate::paths::runtime_version_path;
use crate::types::{RuntimeConfig, RuntimeInstallPlan, RuntimeInstallProgress, RuntimeInstallStep};
use crate::version::{cef_platform_key, channel_preference, major_version, version_sort_key};

pub const DEFAULT_CEF_INDEX_URL: &str = "https://cef-builds.spotifycdn.com/index.json";

pub(crate) fn fetch_cef_index(index_url: &str) -> Result<CefIndex, RuntimeError> {
    let output = run_download_command(index_url, None)?;
    serde_json::from_slice(&output)
        .map_err(|error| RuntimeError::InstallationFailed(error.to_string()))
}

pub(crate) fn archive_url(index_url: &str, archive_name: &str) -> String {
    if archive_name.starts_with("https://") || archive_name.starts_with("http://") {
        return archive_name.to_string();
    }

    let base = index_url
        .rsplit_once('/')
        .map(|(base, _)| base)
        .unwrap_or("");
    if base.is_empty() {
        archive_name.to_string()
    } else {
        format!("{base}/{archive_name}")
    }
}

pub(crate) fn download_file(
    url: &str,
    destination: &Path,
    progress: &mut impl FnMut(RuntimeInstallProgress),
) -> Result<(), RuntimeError> {
    if download_file_with_curl_progress(url, destination, progress).is_ok() {
        return Ok(());
    }
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Downloading,
        None,
        "Downloading runtime",
    ));
    run_download_command(url, Some(destination)).map(|_| ())
}

#[allow(dead_code)]
pub(crate) fn verify_sha1(path: &Path, expected: &str) -> Result<(), RuntimeError> {
    verify_sha1_with_progress(path, expected, &mut |_| {})
}

pub(crate) fn verify_sha1_with_progress(
    path: &Path,
    expected: &str,
    progress: &mut impl FnMut(RuntimeInstallProgress),
) -> Result<(), RuntimeError> {
    let metadata = std::fs::metadata(path)?;
    let total = metadata.len().max(1);
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut read_total = 0_u64;
    let mut last_report = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .unwrap_or_else(std::time::Instant::now);
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        read_total = read_total.saturating_add(read as u64);
        if last_report.elapsed() >= std::time::Duration::from_millis(100) {
            let portion = (read_total as f32 / total as f32).clamp(0.0, 1.0);
            progress(RuntimeInstallProgress::new(
                RuntimeInstallStep::Verifying,
                Some(0.72 + portion * 0.05),
                "Verifying runtime",
            ));
            last_report = std::time::Instant::now();
        }
    }
    let actual = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    actual
        .eq_ignore_ascii_case(expected)
        .then_some(())
        .ok_or_else(|| RuntimeError::IntegrityFailed {
            path: path.to_path_buf(),
        })?;
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Verifying,
        Some(0.78),
        "Verifying runtime",
    ));
    Ok(())
}

pub(crate) fn extract_archive(archive: &Path, destination: &Path) -> Result<(), RuntimeError> {
    std::fs::create_dir_all(destination)?;
    // Canonicalize so the archive path is absolute. Extract with cwd=destination
    // instead of `tar -C <path>`: GNU tar (common via Git for Windows) treats a
    // drive letter in -C as a remote hostname and produces a corrupt/partial tree.
    let archive = std::fs::canonicalize(archive).unwrap_or_else(|_| archive.to_path_buf());
    let status = Command::new("tar")
        .current_dir(destination)
        .arg("-xjf")
        .arg(&archive)
        .status()
        .map_err(RuntimeError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(RuntimeError::InstallationFailed(
            "failed to extract CEF archive with tar".to_string(),
        ))
    }
}

pub(crate) fn first_extracted_runtime_dir(work_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(work_dir)
        .ok()?
        .flatten()
        .find_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (path.is_dir() && name.starts_with("cef_binary_")).then_some(path)
        })
}

pub fn latest_install_plan(config: &RuntimeConfig) -> Result<RuntimeInstallPlan, RuntimeError> {
    let platform = cef_platform_key().ok_or_else(|| {
        RuntimeError::InstallationFailed("unsupported OS or CPU architecture for CEF".to_string())
    })?;
    let index_url = config.index_url.as_deref().unwrap_or(DEFAULT_CEF_INDEX_URL);
    let index = fetch_cef_index(index_url)?;
    let platform_index = index.platforms.get(platform).ok_or_else(|| {
        RuntimeError::InstallationFailed(format!("CEF index does not contain platform {platform}"))
    })?;
    let min_major = crate::MIN_CEF_MAJOR
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(0);

    let mut candidates = platform_index
        .versions
        .iter()
        .filter(|version| major_version(&version.cef_version) >= min_major)
        .filter_map(|version| {
            version
                .files
                .iter()
                .find(|file| file.kind == "standard")
                .map(|file| (version, file))
        })
        .collect::<Vec<_>>();

    candidates.sort_by_key(|(version, _)| {
        (
            channel_preference(version.channel.as_deref()),
            std::cmp::Reverse(version_sort_key(&version.cef_version)),
        )
    });

    let Some((version, file)) = candidates.into_iter().next() else {
        return Err(RuntimeError::NotFound(format!(
            "no Standard CEF build found for {platform} at Chromium {} or newer",
            crate::MIN_CEF_MAJOR,
        )));
    };

    let install_dir = runtime_version_path(&version.cef_version);
    Ok(RuntimeInstallPlan {
        version: version.cef_version.clone(),
        platform: platform.to_string(),
        archive_name: file.name.clone(),
        url: archive_url(index_url, &file.name),
        sha1: file.sha1.clone(),
        install_dir,
    })
}

fn run_download_command(url: &str, destination: Option<&Path>) -> Result<Vec<u8>, RuntimeError> {
    let mut commands = Vec::new();
    if let Some(path) = destination {
        commands.push((
            "curl",
            vec!["-L", "--fail", "-o", path.to_str().unwrap_or_default(), url],
        ));
        commands.push(("wget", vec!["-O", path.to_str().unwrap_or_default(), url]));
    } else {
        commands.push(("curl", vec!["-L", "--fail", url]));
        commands.push(("wget", vec!["-O", "-", url]));
    }

    for (program, args) in commands {
        if let Ok(output) = Command::new(program).args(args).output()
            && output.status.success()
        {
            return Ok(output.stdout);
        }
    }

    Err(RuntimeError::InstallationFailed(
        "could not download runtime; install curl or wget".to_string(),
    ))
}

fn download_file_with_curl_progress(
    url: &str,
    destination: &Path,
    progress: &mut impl FnMut(RuntimeInstallProgress),
) -> Result<(), RuntimeError> {
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Downloading,
        Some(0.05),
        "Downloading runtime",
    ));
    let mut child = Command::new("curl")
        .args([
            "-L",
            "--fail",
            "-o",
            destination.to_string_lossy().as_ref(),
            url,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        for line in reader.split(b'\r').flatten() {
            if let Some(percent) = parse_curl_percent(&line) {
                progress(RuntimeInstallProgress::new(
                    RuntimeInstallStep::Downloading,
                    Some(0.05 + (percent / 100.0) * 0.65),
                    format!("Downloading runtime ({percent:.0}%)"),
                ));
            }
        }
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(RuntimeError::InstallationFailed(
            "curl download failed".to_string(),
        ))
    }
}

fn parse_curl_percent(line: &[u8]) -> Option<f32> {
    let text = String::from_utf8_lossy(line);
    let token = text.split_whitespace().next()?;
    let percent = token.parse::<f32>().ok()?;
    (0.0..=100.0).contains(&percent).then_some(percent)
}

#[derive(Deserialize)]
pub(crate) struct CefIndex {
    #[serde(flatten)]
    platforms: BTreeMap<String, CefPlatformIndex>,
}

#[derive(Deserialize)]
struct CefPlatformIndex {
    versions: Vec<CefVersion>,
}

#[derive(Deserialize)]
struct CefVersion {
    cef_version: String,
    #[serde(default)]
    channel: Option<String>,
    files: Vec<CefFile>,
}

#[derive(Deserialize)]
struct CefFile {
    name: String,
    sha1: String,
    #[serde(rename = "type")]
    kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_urls_follow_index_location() {
        assert_eq!(
            archive_url("https://example.com/cef/index.json", "cef.tar.bz2"),
            "https://example.com/cef/cef.tar.bz2"
        );
        assert_eq!(
            archive_url(
                "https://example.com/cef/index.json",
                "https://cdn.example/cef.tar.bz2"
            ),
            "https://cdn.example/cef.tar.bz2"
        );
    }
}
