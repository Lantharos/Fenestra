use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

pub const DEFAULT_CEF_INDEX_URL: &str = "https://cef-builds.spotifycdn.com/index.json";
const INSTALL_LOCK_TIMEOUT: Duration = Duration::from_secs(600);
const INSTALL_LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime not found: {0}")]
    NotFound(String),
    #[error("runtime at {path} has version {found}, minimum required is {required}")]
    VersionTooLow {
        path: PathBuf,
        found: String,
        required: String,
    },
    #[error("runtime integrity check failed for {path}")]
    IntegrityFailed { path: PathBuf },
    #[error("runtime installation failed: {0}")]
    InstallationFailed(String),
    #[error("runtime downloads are disabled by configuration")]
    DownloadsDisabled,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    SystemRequired,
    SystemPreferred,
    UserPreferred,
    SharedPreferred,
    Bundled,
    Disabled,
}

impl RuntimeMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system-required" => Some(Self::SystemRequired),
            "system-preferred" => Some(Self::SystemPreferred),
            "user-preferred" => Some(Self::UserPreferred),
            "shared-preferred" => Some(Self::SharedPreferred),
            "bundled" => Some(Self::Bundled),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimePackage {
    Minimal,
    Client,
    #[default]
    Standard,
}

impl RuntimePackage {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "minimal" => Some(Self::Minimal),
            "client" => Some(Self::Client),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Client => "client",
            Self::Standard => "standard",
        }
    }

    fn install_suffix(self) -> &'static str {
        match self {
            Self::Minimal => "",
            Self::Client => "-client",
            Self::Standard => "-standard",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub version: String,
    pub location: RuntimeLocation,
    pub verified: bool,
    pub package: RuntimePackage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeLocation {
    System(PathBuf),
    UserLocal(PathBuf),
    Bundled(PathBuf),
}

impl RuntimeLocation {
    pub fn path(&self) -> &Path {
        match self {
            Self::System(p) | Self::UserLocal(p) | Self::Bundled(p) => p,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub mode: RuntimeMode,
    pub package: RuntimePackage,
    pub min_version: String,
    pub index_url: Option<String>,
    pub allow_user_install: bool,
    pub allow_bundled: bool,
    pub bundled_dir: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::SharedPreferred,
            package: RuntimePackage::Standard,
            min_version: "144".to_string(),
            index_url: None,
            allow_user_install: true,
            allow_bundled: true,
            bundled_dir: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeInstallPlan {
    pub package: RuntimePackage,
    pub version: String,
    pub platform: String,
    pub archive_name: String,
    pub url: String,
    pub sha1: String,
    pub install_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeInstallStep {
    Preparing,
    RemovingOldRuntime,
    Downloading,
    Verifying,
    Extracting,
    Installing,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeInstallProgress {
    pub step: RuntimeInstallStep,
    pub fraction: Option<f32>,
    pub message: String,
}

impl RuntimeInstallProgress {
    pub fn new(
        step: RuntimeInstallStep,
        fraction: Option<f32>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            step,
            fraction: fraction.map(|value| value.clamp(0.0, 1.0)),
            message: message.into(),
        }
    }
}

pub fn system_runtime_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
            .join("Mullion/Runtime/cef");
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/Mullion/runtimes/cef")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        PathBuf::from("/usr/lib/mullion/cef")
    }
}

pub fn user_runtime_path() -> PathBuf {
    user_data_dir().join("mullion").join("runtimes").join("cef")
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

pub fn runtime_version_path(package: RuntimePackage, version: &str) -> PathBuf {
    user_runtime_path().join(format!("{}{}", version, package.install_suffix()))
}

pub fn detect_runtime(config: &RuntimeConfig) -> Vec<RuntimeInfo> {
    let mut runtimes = Vec::new();
    collect_runtime_dirs(
        RuntimeLocationKind::System,
        system_runtime_path(),
        &mut runtimes,
    );
    collect_runtime_dirs(
        RuntimeLocationKind::UserLocal,
        user_runtime_path(),
        &mut runtimes,
    );
    if config.allow_bundled
        && let Some(dir) = &config.bundled_dir
    {
        collect_runtime_dirs(
            RuntimeLocationKind::Bundled,
            bundled_runtime_path(dir),
            &mut runtimes,
        );
    }
    runtimes
}

pub fn resolve_runtime(config: &RuntimeConfig) -> Result<RuntimeInfo, RuntimeError> {
    let runtimes = detect_runtime(config);
    select_runtime(config, runtimes).ok_or_else(|| {
        RuntimeError::NotFound(format!(
            "no compatible CEF runtime found for mode {:?}",
            config.mode
        ))
    })
}

pub fn ensure_runtime(config: &RuntimeConfig) -> Result<RuntimeInfo, RuntimeError> {
    match resolve_runtime(config) {
        Ok(runtime) => Ok(runtime),
        Err(_) if config.allow_user_install && should_install_user_runtime(config) => {
            install_user_runtime(config)
        }
        Err(error) => Err(error),
    }
}

pub fn install_user_runtime(config: &RuntimeConfig) -> Result<RuntimeInfo, RuntimeError> {
    install_user_runtime_with_progress(config, |_| {})
}

pub fn install_user_runtime_with_progress(
    config: &RuntimeConfig,
    progress: impl FnMut(RuntimeInstallProgress),
) -> Result<RuntimeInfo, RuntimeError> {
    install_user_runtime_inner(config, progress, false)
}

pub fn update_user_runtime_with_progress(
    config: &RuntimeConfig,
    progress: impl FnMut(RuntimeInstallProgress),
) -> Result<RuntimeInfo, RuntimeError> {
    install_user_runtime_inner(config, progress, true)
}

fn install_user_runtime_inner(
    config: &RuntimeConfig,
    mut progress: impl FnMut(RuntimeInstallProgress),
    require_latest: bool,
) -> Result<RuntimeInfo, RuntimeError> {
    if !config.allow_user_install {
        return Err(RuntimeError::DownloadsDisabled);
    }
    std::fs::create_dir_all(user_runtime_path())?;
    let _lock = RuntimeInstallLock::acquire(&mut progress)?;
    if !require_latest && let Ok(runtime) = resolve_runtime(config) {
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::Complete,
            Some(1.0),
            "Runtime ready",
        ));
        return Ok(runtime);
    }
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Preparing,
        None,
        "Preparing runtime install",
    ));
    remove_user_minimal_runtime_if_client_requested_with_progress(config, &mut progress)?;

    let plan = latest_install_plan(config)?;
    if plan.install_dir.is_dir() {
        return Ok(RuntimeInfo {
            package: config.package,
            version: plan.version,
            location: RuntimeLocation::UserLocal(plan.install_dir),
            verified: true,
        });
    }

    let work_dir = user_runtime_path().join(".installing");
    if work_dir.exists() {
        std::fs::remove_dir_all(&work_dir)?;
    }
    std::fs::create_dir_all(&work_dir)?;

    let archive_path = work_dir.join(&plan.archive_name);
    download_file(&plan.url, &archive_path, &mut progress)?;
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Verifying,
        None,
        "Verifying CEF archive",
    ));
    verify_sha1(&archive_path, &plan.sha1)?;
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Extracting,
        None,
        "Extracting CEF runtime",
    ));
    extract_archive(&archive_path, &work_dir)?;

    let extracted = first_extracted_runtime_dir(&work_dir).ok_or_else(|| {
        RuntimeError::InstallationFailed(
            "download did not contain a CEF runtime directory".to_string(),
        )
    })?;
    if plan.install_dir.exists() {
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::RemovingOldRuntime,
            None,
            "Removing previous runtime",
        ));
        std::fs::remove_dir_all(&plan.install_dir)?;
    }
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Installing,
        None,
        "Installing CEF runtime",
    ));
    std::fs::rename(&extracted, &plan.install_dir)?;
    std::fs::write(plan.install_dir.join("VERSION"), &plan.version)?;
    let _ = std::fs::remove_dir_all(&work_dir);
    progress(RuntimeInstallProgress::new(
        RuntimeInstallStep::Complete,
        Some(1.0),
        "Runtime ready",
    ));

    Ok(RuntimeInfo {
        package: config.package,
        version: plan.version,
        location: RuntimeLocation::UserLocal(plan.install_dir),
        verified: true,
    })
}

pub fn remove_user_minimal_runtime_if_client_requested(
    config: &RuntimeConfig,
) -> Result<(), RuntimeError> {
    remove_user_minimal_runtime_if_client_requested_with_progress(config, |_| {})
}

pub fn prune_user_runtimes(
    config: &RuntimeConfig,
    keep_latest: usize,
) -> Result<usize, RuntimeError> {
    std::fs::create_dir_all(user_runtime_path())?;
    let _lock = RuntimeInstallLock::acquire(|_| {})?;
    let base = user_runtime_path();
    if !base.is_dir() {
        return Ok(0);
    }

    let mut runtimes = std::fs::read_dir(base)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && is_runtime_dir(path))
        .filter(|path| detect_package(path) == config.package)
        .collect::<Vec<_>>();
    runtimes.sort_by_key(|path| std::cmp::Reverse(runtime_sort_key(path)));

    let mut removed = 0;
    for path in runtimes.into_iter().skip(keep_latest.max(1)) {
        std::fs::remove_dir_all(path)?;
        removed += 1;
    }
    Ok(removed)
}

pub fn remove_user_runtime_version(
    config: &RuntimeConfig,
    version: &str,
) -> Result<bool, RuntimeError> {
    std::fs::create_dir_all(user_runtime_path())?;
    let _lock = RuntimeInstallLock::acquire(|_| {})?;
    let base = user_runtime_path();
    if !base.is_dir() {
        return Ok(false);
    }

    let mut removed = false;
    for path in std::fs::read_dir(base)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && is_runtime_dir(path))
        .filter(|path| detect_package(path) == config.package)
        .filter(|path| detect_version(path) == version)
    {
        std::fs::remove_dir_all(path)?;
        removed = true;
    }
    Ok(removed)
}

struct RuntimeInstallLock {
    path: PathBuf,
}

impl RuntimeInstallLock {
    fn acquire(mut progress: impl FnMut(RuntimeInstallProgress)) -> Result<Self, RuntimeError> {
        let base = user_runtime_path();
        std::fs::create_dir_all(&base)?;
        let path = base.join(".install.lock");
        let started = Instant::now();
        let mut announced_wait = false;

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
                    if !announced_wait {
                        progress(RuntimeInstallProgress::new(
                            RuntimeInstallStep::Preparing,
                            None,
                            "Waiting for another Mullion runtime install",
                        ));
                        announced_wait = true;
                    }
                    if started.elapsed() >= INSTALL_LOCK_TIMEOUT {
                        return Err(RuntimeError::InstallationFailed(format!(
                            "timed out waiting for runtime install lock at {}",
                            path.display()
                        )));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for RuntimeInstallLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn remove_user_minimal_runtime_if_client_requested_with_progress(
    config: &RuntimeConfig,
    mut progress: impl FnMut(RuntimeInstallProgress),
) -> Result<(), RuntimeError> {
    if config.package == RuntimePackage::Minimal {
        return Ok(());
    }

    let base = user_runtime_path();
    if !base.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(base)? {
        let path = entry?.path();
        if !path.is_dir() || detect_package(&path) != RuntimePackage::Minimal {
            continue;
        }
        progress(RuntimeInstallProgress::new(
            RuntimeInstallStep::RemovingOldRuntime,
            None,
            "Removing minimal runtime",
        ));
        std::fs::remove_dir_all(path)?;
    }

    Ok(())
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
    let min_major = config
        .min_version
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
                .find(|file| file.kind == config.package.as_str())
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
            "no {} CEF build found for {platform} at Chromium {} or newer",
            config.package.as_str(),
            config.min_version
        )));
    };

    let install_dir = runtime_version_path(config.package, &version.cef_version);
    Ok(RuntimeInstallPlan {
        package: config.package,
        version: version.cef_version.clone(),
        platform: platform.to_string(),
        archive_name: file.name.clone(),
        url: archive_url(index_url, &file.name),
        sha1: file.sha1.clone(),
        install_dir,
    })
}

fn detect_version(runtime_dir: &Path) -> String {
    let version_file = runtime_dir.join("VERSION");
    std::fs::read_to_string(version_file)
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

#[derive(Clone, Copy)]
enum RuntimeLocationKind {
    System,
    UserLocal,
    Bundled,
}

fn collect_runtime_dirs(kind: RuntimeLocationKind, base: PathBuf, runtimes: &mut Vec<RuntimeInfo>) {
    if !base.is_dir() {
        return;
    }

    if is_runtime_dir(&base) {
        runtimes.push(runtime_info(kind, base));
        return;
    }

    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && is_runtime_dir(&path) {
                runtimes.push(runtime_info(kind, path));
            }
        }
    }
}

fn runtime_info(kind: RuntimeLocationKind, path: PathBuf) -> RuntimeInfo {
    let location = match kind {
        RuntimeLocationKind::System => RuntimeLocation::System(path.clone()),
        RuntimeLocationKind::UserLocal => RuntimeLocation::UserLocal(path.clone()),
        RuntimeLocationKind::Bundled => RuntimeLocation::Bundled(path.clone()),
    };
    RuntimeInfo {
        package: detect_package(&path),
        version: detect_version(&path),
        location,
        verified: is_runtime_dir(&path),
    }
}

fn detect_package(runtime_dir: &Path) -> RuntimePackage {
    if runtime_is_standard(runtime_dir) {
        RuntimePackage::Standard
    } else if runtime_is_launchable_client(runtime_dir) {
        RuntimePackage::Client
    } else {
        RuntimePackage::Minimal
    }
}

fn is_runtime_dir(path: &Path) -> bool {
    path.join("VERSION").is_file()
        || path.join("Release").is_dir()
        || path.join("Resources").is_dir()
        || path.join("libcef.so").is_file()
        || path.join("libcef.dll").is_file()
        || path.join("Chromium Embedded Framework.framework").is_dir()
}

pub fn has_host_binary(path: &Path) -> bool {
    launchable_host_candidates(path)
        .into_iter()
        .any(|candidate| candidate.is_file())
}

pub fn host_candidates(runtime_dir: &Path) -> Vec<PathBuf> {
    launchable_host_candidates(runtime_dir)
}

pub fn launchable_host_candidates(runtime_dir: &Path) -> Vec<PathBuf> {
    vec![
        runtime_dir.join("cefclient"),
        runtime_dir.join("Release").join("cefclient"),
        runtime_dir.join("bin").join("cefclient"),
        runtime_dir.join("cefsimple"),
        runtime_dir.join("Release").join("cefsimple"),
        runtime_dir.join("cefclient.exe"),
        runtime_dir.join("Release").join("cefclient.exe"),
        runtime_dir.join("cefsimple.exe"),
        runtime_dir.join("Release").join("cefsimple.exe"),
        runtime_dir
            .join("cefclient.app")
            .join("Contents")
            .join("MacOS")
            .join("cefclient"),
        runtime_dir
            .join("cefsimple.app")
            .join("Contents")
            .join("MacOS")
            .join("cefsimple"),
    ]
}

fn runtime_is_launchable_client(runtime_dir: &Path) -> bool {
    runtime_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(RuntimePackage::Client.install_suffix()))
        && has_host_binary(runtime_dir)
}

fn runtime_is_standard(runtime_dir: &Path) -> bool {
    runtime_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(RuntimePackage::Standard.install_suffix()))
        && runtime_dir.join("include").is_dir()
        && runtime_dir.join("libcef_dll").is_dir()
        && runtime_dir.join("Release").join("libcef.so").is_file()
}

fn select_runtime(config: &RuntimeConfig, runtimes: Vec<RuntimeInfo>) -> Option<RuntimeInfo> {
    let mut compatible = runtimes
        .into_iter()
        .filter(|runtime| version_satisfies(&runtime.version, &config.min_version))
        .filter(|runtime| runtime.package == config.package)
        .filter(|runtime| location_allowed(config.mode, &runtime.location))
        .collect::<Vec<_>>();

    compatible.sort_by_key(|runtime| {
        (
            runtime_priority(config.mode, &runtime.location),
            std::cmp::Reverse(version_sort_key(&runtime.version)),
        )
    });
    compatible.into_iter().next()
}

fn location_allowed(mode: RuntimeMode, location: &RuntimeLocation) -> bool {
    match mode {
        RuntimeMode::SystemRequired => matches!(location, RuntimeLocation::System(_)),
        RuntimeMode::Bundled => matches!(location, RuntimeLocation::Bundled(_)),
        RuntimeMode::Disabled => false,
        RuntimeMode::SystemPreferred
        | RuntimeMode::UserPreferred
        | RuntimeMode::SharedPreferred => true,
    }
}

fn runtime_priority(mode: RuntimeMode, location: &RuntimeLocation) -> u8 {
    match mode {
        RuntimeMode::SystemRequired => match location {
            RuntimeLocation::System(_) => 0,
            _ => 9,
        },
        RuntimeMode::SystemPreferred => match location {
            RuntimeLocation::System(_) => 0,
            RuntimeLocation::UserLocal(_) => 1,
            RuntimeLocation::Bundled(_) => 2,
        },
        RuntimeMode::UserPreferred | RuntimeMode::SharedPreferred => match location {
            RuntimeLocation::UserLocal(_) => 0,
            RuntimeLocation::System(_) => 1,
            RuntimeLocation::Bundled(_) => 2,
        },
        RuntimeMode::Bundled => match location {
            RuntimeLocation::Bundled(_) => 0,
            _ => 9,
        },
        RuntimeMode::Disabled => 9,
    }
}

fn should_install_user_runtime(config: &RuntimeConfig) -> bool {
    matches!(
        config.mode,
        RuntimeMode::SystemPreferred | RuntimeMode::UserPreferred | RuntimeMode::SharedPreferred
    )
}

fn version_satisfies(found: &str, required: &str) -> bool {
    found != "unknown" && major_version(found) >= major_version(required)
}

fn runtime_sort_key(path: &Path) -> Vec<u32> {
    version_sort_key(&detect_version(path))
}

fn version_sort_key(version: &str) -> Vec<u32> {
    version
        .split(['.', '+', '-', '_'])
        .filter_map(|part| part.parse::<u32>().ok())
        .collect()
}

fn channel_preference(channel: Option<&str>) -> u8 {
    match channel {
        Some("stable") | None => 0,
        Some("beta") => 1,
        Some("dev") | Some("canary") => 2,
        _ => 3,
    }
}

fn major_version(version: &str) -> u32 {
    version
        .split(['.', '+'])
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(0)
}

fn cef_platform_key() -> Option<&'static str> {
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

fn lock_is_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= INSTALL_LOCK_STALE_AFTER)
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct CefIndex {
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

fn fetch_cef_index(index_url: &str) -> Result<CefIndex, RuntimeError> {
    let output = run_download_command(index_url, None)?;
    serde_json::from_slice(&output)
        .map_err(|error| RuntimeError::InstallationFailed(error.to_string()))
}

fn archive_url(index_url: &str, archive_name: &str) -> String {
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

fn download_file(
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
        "Downloading CEF runtime",
    ));
    run_download_command(url, Some(destination)).map(|_| ())
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
        Some(0.0),
        "Downloading CEF runtime",
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
                    Some(percent / 100.0),
                    format!("Downloading CEF runtime ({percent:.0}%)"),
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

fn verify_sha1(path: &Path, expected: &str) -> Result<(), RuntimeError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
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
    actual
        .eq_ignore_ascii_case(expected)
        .then_some(())
        .ok_or_else(|| RuntimeError::IntegrityFailed {
            path: path.to_path_buf(),
        })
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), RuntimeError> {
    let status = Command::new("tar")
        .args([
            "-xjf",
            archive.to_string_lossy().as_ref(),
            "-C",
            destination.to_string_lossy().as_ref(),
        ])
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

fn first_extracted_runtime_dir(work_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(work_dir)
        .ok()?
        .flatten()
        .find_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (path.is_dir() && name.starts_with("cef_binary_")).then_some(path)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_mode_round_trips() {
        assert_eq!(
            RuntimeMode::SharedPreferred,
            RuntimeMode::parse("shared-preferred").unwrap()
        );
        assert_eq!(
            RuntimeMode::SystemRequired,
            RuntimeMode::parse("system-required").unwrap()
        );
        assert!(RuntimeMode::parse("invalid").is_none());
    }

    #[test]
    fn runtime_package_round_trips() {
        assert_eq!(
            RuntimePackage::Client,
            RuntimePackage::parse("client").unwrap()
        );
        assert_eq!(
            RuntimePackage::Minimal,
            RuntimePackage::parse("minimal").unwrap()
        );
        assert_eq!(
            RuntimePackage::Standard,
            RuntimePackage::parse("standard").unwrap()
        );
        assert_eq!(RuntimePackage::Standard.as_str(), "standard");
        assert!(RuntimePackage::parse("browser").is_none());
    }

    #[test]
    fn runtime_config_has_sane_defaults() {
        let config = RuntimeConfig::default();
        assert_eq!(config.mode, RuntimeMode::SharedPreferred);
        assert_eq!(config.package, RuntimePackage::Standard);
        assert_eq!(config.index_url, None);
        assert!(config.allow_user_install);
        assert!(config.allow_bundled);
    }

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

    #[test]
    fn runtime_location_extracts_path() {
        let path = PathBuf::from("/usr/lib/mullion/cef");
        let loc = RuntimeLocation::System(path.clone());
        assert_eq!(loc.path(), path);
    }

    #[test]
    fn detect_runtime_skips_missing_dirs() {
        let config = RuntimeConfig::default();
        let runtimes = detect_runtime(&config);
        assert!(runtimes.is_empty() || runtimes.iter().all(|r| r.location.path().is_dir()));
    }

    #[test]
    fn version_checks_use_major_version() {
        assert!(version_satisfies(
            "147.0.14+gabc+chromium-147.0.7727.138",
            "126"
        ));
        assert!(!version_satisfies(
            "101.0.18+gabc+chromium-101.0.4951.67",
            "126"
        ));
    }
}
