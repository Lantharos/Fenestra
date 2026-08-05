use std::{fs, path::Path};

use serde::Deserialize;

use super::MullionWindow;
use crate::error::{MullionError, MullionResult};

#[derive(Debug, Default, Deserialize)]
struct MullionFile {
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
    #[serde(default)]
    allowed_origins: Vec<String>,
}

impl MullionWindow {
    /// Applies `[app]` / `[web]` fields from a `Mullion.toml` next to the crate.
    ///
    /// Bridge handlers, chrome, and size stay in Rust; identity and content
    /// paths can live in the manifest shared with the CLI/bundler.
    pub fn from_manifest(path: impl AsRef<Path>) -> MullionResult<Self> {
        Self::new().with_manifest(path)
    }

    pub fn with_manifest(mut self, path: impl AsRef<Path>) -> MullionResult<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|error| {
            MullionError::Io(format!(
                "failed to read Mullion.toml at {}: {error}",
                path.display()
            ))
        })?;
        let file: MullionFile = toml::from_str(&text).map_err(|error| {
            MullionError::Io(format!(
                "failed to parse Mullion.toml at {}: {error}",
                path.display()
            ))
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
        if let Some(dev_url) = file.web.dev_url {
            self = self.dev_url(dev_url);
        }
        for origin in file.web.allowed_origins {
            self = self.allowed_origin(origin);
        }
        Ok(self)
    }
}
