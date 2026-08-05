use std::{fs, path::Path};

use serde::Deserialize;

use super::MullionWindow;
use super::dev::{parse_localhost_port, vite_dev_command, vite_dev_url};
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
    dev_port: Option<u16>,
    dev_command: Option<String>,
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

        let mut allowed_origins = file.web.allowed_origins;
        let explicit_dev_command = file
            .web
            .dev_command
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let mut dev_url = file
            .web
            .dev_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if dev_url.is_none() {
            if let Some(port) = file.web.dev_port {
                dev_url = Some(vite_dev_url(port));
            }
        }
        let port = file
            .web
            .dev_port
            .or_else(|| dev_url.as_deref().and_then(parse_localhost_port));
        if let Some(url) = &dev_url {
            if allowed_origins.is_empty() {
                allowed_origins.push(url.clone());
            }
            self = self.dev_url(url.clone());
        }
        if let Some(command) = explicit_dev_command {
            self = self.dev_command(command);
        } else if let Some(port) = port {
            self = self.dev_command(vite_dev_command(port, "bun"));
        }
        for origin in allowed_origins {
            self = self.allowed_origin(origin);
        }
        Ok(self)
    }
}
