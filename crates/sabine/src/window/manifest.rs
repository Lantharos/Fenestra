use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::SabineWindow;
use crate::error::{SabineError, SabineResult};

const MANIFEST_ENV: &str = "SABINE_MANIFEST_PATH";
const APP_ID_ENV: &str = "SABINE_APP_ID";
const WEB_ENTRY_ENV: &str = "SABINE_WEB_ENTRY";

#[derive(Debug, Default, Deserialize)]
struct SabineFile {
    #[serde(default)]
    app: AppSection,
    #[serde(default)]
    web: WebSection,
}

#[derive(Debug, Default, Deserialize)]
struct AppSection {
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WebSection {
    entry: Option<String>,
    url: Option<String>,
    #[serde(default)]
    allowed_origins: Vec<String>,
}

impl SabineWindow {
    /// Applies `[app]` and production `[web]` fields from a `Sabine.toml`.
    ///
    /// Bridge handlers, chrome, and size stay in Rust; identity and content
    /// paths can live in the manifest shared with the CLI/bundler.
    pub fn from_manifest(path: impl AsRef<Path>) -> SabineResult<Self> {
        Self::new().with_manifest(path)
    }

    pub fn with_manifest(mut self, path: impl AsRef<Path>) -> SabineResult<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| SabineError::ManifestRead {
            path: path.to_path_buf(),
            source,
        })?;
        let file: SabineFile =
            toml::from_str(&text).map_err(|source| SabineError::ManifestParse {
                path: path.to_path_buf(),
                source,
            })?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        if let Some(id) = file.app.id {
            self = self.app_id(id);
        }
        if let Some(name) = file.app.name {
            self = self.title(name);
        }
        if let Some(version) = file.app.version {
            self = self.app_version(version);
        }
        if let Some(entry) = file.web.entry {
            let entry_path = base.join(&entry);
            self = self.entry(entry_path.display().to_string());
        }
        if let Some(url) = file.web.url {
            self = self.url(url);
        }

        for origin in file.web.allowed_origins {
            self = self.allowed_origin(origin);
        }
        Ok(self)
    }

    pub(super) fn with_framework_config(mut self) -> SabineResult<Self> {
        if let Some(path) = framework_manifest_path() {
            self = self.with_manifest(path)?;
        }
        if let Some(app_id) = nonempty_env(APP_ID_ENV) {
            self = self.app_id(app_id);
        }
        if let Some(entry) = nonempty_env(WEB_ENTRY_ENV) {
            self.config.entry = Some(entry);
            self.config.url = None;
            self.config.dev_url = None;
        }
        Ok(self)
    }
}

fn framework_manifest_path() -> Option<PathBuf> {
    if let Some(path) = nonempty_env(MANIFEST_ENV) {
        return Some(PathBuf::from(path));
    }

    if let Ok(executable) = env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let stem = executable.file_stem().and_then(|stem| stem.to_str());
        let mut candidates = vec![
            directory.join("resources/Sabine.toml"),
            directory.join("../Resources/Sabine.toml"),
        ];
        if let Some(stem) = stem {
            candidates.push(
                directory
                    .join("../share/sabine/manifests")
                    .join(format!("{stem}.toml")),
            );
        }
        if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
            return Some(path);
        }
    }

    env::current_dir()
        .ok()
        .map(|directory| directory.join("Sabine.toml"))
        .filter(|path| path.is_file())
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
