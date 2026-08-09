use std::{fs, path::Path};

use serde::Deserialize;

use super::SabineWindow;
use super::dev::vite_dev_url;
use crate::error::{SabineError, SabineResult};

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
    dev_url: Option<String>,
    dev_port: Option<u16>,
    #[serde(default)]
    allowed_origins: Vec<String>,
}

impl SabineWindow {
    /// Applies `[app]` / `[web]` fields from a `Sabine.toml` next to the crate.
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

        let mut allowed_origins = file.web.allowed_origins;
        let mut dev_url = file
            .web
            .dev_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if dev_url.is_none()
            && let Some(port) = file.web.dev_port
        {
            dev_url = Some(vite_dev_url(port));
        }
        if let Some(url) = &dev_url {
            if allowed_origins.is_empty() {
                allowed_origins.push(url.clone());
            }
            self = self.dev_url(url.clone());
        }
        for origin in allowed_origins {
            self = self.allowed_origin(origin);
        }
        Ok(self)
    }
}
