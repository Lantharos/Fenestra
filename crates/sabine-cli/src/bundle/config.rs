use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Debug)]
pub(super) struct BundleApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub icon: Option<PathBuf>,
    pub mime_types: Vec<String>,
    pub cargo_manifest: PathBuf,
    pub source_dir: PathBuf,
    pub cargo_package: String,
    pub web: Option<WebBundle>,
}

#[derive(Debug)]
pub(super) struct WebBundle {
    pub root: PathBuf,
    pub dist: PathBuf,
    pub entry: PathBuf,
    pub build_command: Option<String>,
    pub has_local_assets: bool,
    pub url: Option<String>,
    pub dev_url: Option<String>,
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct ConfigOverrides {
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub web_build: Option<String>,
    pub web_root: Option<PathBuf>,
    pub web_dist: Option<PathBuf>,
}

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
    icon: Option<String>,
    #[serde(default)]
    mime_types: Vec<String>,
    cargo_manifest: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WebSection {
    root: Option<String>,
    dist: Option<String>,
    entry: Option<String>,
    build: Option<String>,
    url: Option<String>,
    dev_url: Option<String>,
    #[serde(default)]
    allowed_origins: Vec<String>,
}

pub(super) fn resolve_app(source: &Path, overrides: ConfigOverrides) -> Result<BundleApp, String> {
    let source_dir = absolute_path(source)?;
    let sabine = read_sabine_file(&source_dir)?;
    let cargo_manifest = sabine
        .app
        .cargo_manifest
        .as_ref()
        .map(|path| source_dir.join(path))
        .unwrap_or_else(|| source_dir.join("Cargo.toml"));
    let cargo_package = cargo_package_name(&cargo_manifest)
        .ok_or_else(|| format!("missing package name in {}", cargo_manifest.display()))?;
    let web = resolve_web(&source_dir, &sabine.web, &overrides)?;

    let name = overrides
        .name
        .or(sabine.app.name)
        .unwrap_or_else(|| cargo_package.replace('_', " "));
    let id = overrides
        .id
        .or(sabine.app.id)
        .unwrap_or_else(|| format!("dev.sabine.{}", sanitize_id(&name)));
    let version = overrides.version.or(sabine.app.version).unwrap_or_else(|| {
        cargo_package_version(&cargo_manifest).unwrap_or_else(|| "0.1.0".to_string())
    });
    let icon = sabine
        .app
        .icon
        .map(|icon| source_dir.join(icon))
        .or_else(|| detect_icon(&source_dir));

    Ok(BundleApp {
        id: sanitize_id(&id),
        name,
        version,
        icon,
        mime_types: sabine.app.mime_types,
        cargo_manifest,
        source_dir,
        cargo_package,
        web,
    })
}

fn resolve_web(
    source_dir: &Path,
    config: &WebSection,
    overrides: &ConfigOverrides,
) -> Result<Option<WebBundle>, String> {
    let url = config.url.clone();
    let dev_url = config.dev_url.clone();
    let allowed_origins = config.allowed_origins.clone();
    let configured_root = overrides
        .web_root
        .clone()
        .or_else(|| config.root.as_ref().map(PathBuf::from));
    let configured_entry = config.entry.as_ref().map(PathBuf::from);
    let configured_dist = overrides
        .web_dist
        .clone()
        .or_else(|| config.dist.as_ref().map(PathBuf::from));
    let has_explicit_local_assets =
        configured_root.is_some() || configured_entry.is_some() || configured_dist.is_some();
    let has_remote_url = url.is_some() || dev_url.is_some() || !allowed_origins.is_empty();
    let package_root = configured_root
        .as_deref()
        .map(|root| source_dir.join(root))
        .or_else(|| {
            (!has_remote_url)
                .then(|| detect_package_root(source_dir))
                .flatten()
        });
    let entry = config.entry.as_ref().map(PathBuf::from).or_else(|| {
        (!has_remote_url)
            .then(|| default_web_entry(source_dir))
            .flatten()
    });
    let dist = configured_dist.map(|path| source_dir.join(path));

    if package_root.is_none() && entry.is_none() && dist.is_none() && !has_remote_url {
        return Ok(None);
    }

    let root = package_root
        .or_else(|| {
            entry
                .as_ref()
                .and_then(|entry| source_dir.join(entry).parent().map(Path::to_path_buf))
        })
        .unwrap_or_else(|| source_dir.join("ui"));
    let entry = entry
        .map(|entry| source_dir.join(entry))
        .unwrap_or_else(|| root.join("index.html"));
    let dist = dist.unwrap_or_else(|| {
        if root.join("dist").exists() || root.join("package.json").exists() {
            root.join("dist")
        } else {
            entry.parent().unwrap_or(&root).to_path_buf()
        }
    });
    let build_command = overrides
        .web_build
        .clone()
        .or_else(|| config.build.clone())
        .or_else(|| {
            if has_explicit_local_assets || !has_remote_url {
                detect_web_build_command(&root)
            } else {
                None
            }
        });
    let has_local_assets = has_explicit_local_assets
        || entry.is_file()
        || dist.exists()
        || root.join("package.json").is_file() && !has_remote_url;

    Ok(Some(WebBundle {
        root,
        dist,
        entry,
        build_command,
        has_local_assets,
        url,
        dev_url,
        allowed_origins,
    }))
}

fn read_sabine_file(source_dir: &Path) -> Result<SabineFile, String> {
    let path = source_dir.join("Sabine.toml");
    if !path.exists() {
        return Ok(SabineFile::default());
    }
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    toml::from_str(&text).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn detect_package_root(source_dir: &Path) -> Option<PathBuf> {
    crate::web_detect::detect_package_root(source_dir)
}

fn default_web_entry(source_dir: &Path) -> Option<PathBuf> {
    [
        "ui/index.html",
        "web/index.html",
        "frontend/index.html",
        "index.html",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|entry| source_dir.join(entry).is_file())
}

fn detect_icon(source_dir: &Path) -> Option<PathBuf> {
    [
        "static/icon.svg",
        "static/favicon.svg",
        "src/lib/assets/favicon.svg",
        "favicon.svg",
        "icon.svg",
        "icon.png",
        "icons/icon.svg",
        "icons/icon.png",
        "desktop/icons/icon.svg",
        "static/icon.png",
        "desktop/icons/icon.png",
    ]
    .iter()
    .map(|icon| source_dir.join(icon))
    .find(|icon| icon.is_file())
}

fn detect_web_build_command(root: &Path) -> Option<String> {
    crate::web_detect::detect_web_build_command(root)
}

fn cargo_package_name(path: &Path) -> Option<String> {
    cargo_package_value(path, "name")
}

fn cargo_package_version(path: &Path) -> Option<String> {
    cargo_package_value(path, "version")
}

fn cargo_package_value(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && trimmed.starts_with(key) {
            return toml_string_value(trimmed);
        }
    }
    None
}

fn toml_string_value(line: &str) -> Option<String> {
    let value = line.split_once('=')?.1.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .map(ToOwned::to_owned)
}

fn sanitize_id(value: &str) -> String {
    let output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        "app".to_string()
    } else {
        output
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("source path does not exist: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_only_web_config_does_not_require_local_assets() {
        let source = PathBuf::from("/tmp/sabine-remote-only-config-test");
        let web = resolve_web(
            &source,
            &WebSection {
                url: Some("https://raday.lantharos.com".to_string()),
                allowed_origins: vec!["https://api.lantharos.com".to_string()],
                ..WebSection::default()
            },
            &ConfigOverrides::default(),
        )
        .unwrap()
        .unwrap();

        assert!(!web.has_local_assets);
        assert_eq!(web.url.as_deref(), Some("https://raday.lantharos.com"));
        assert_eq!(
            web.allowed_origins,
            vec!["https://api.lantharos.com".to_string()]
        );
    }

    #[test]
    fn explicit_local_assets_are_kept_for_site_backed_apps() {
        let source = PathBuf::from("/tmp/sabine-site-backed-config-test");
        let web = resolve_web(
            &source,
            &WebSection {
                root: Some("ui".to_string()),
                dist: Some("ui/dist".to_string()),
                entry: Some("ui/dist/index.html".to_string()),
                url: Some("https://raday.lantharos.com".to_string()),
                ..WebSection::default()
            },
            &ConfigOverrides::default(),
        )
        .unwrap()
        .unwrap();

        assert!(web.has_local_assets);
        assert_eq!(web.root, source.join("ui"));
        assert_eq!(web.dist, source.join("ui/dist"));
        assert_eq!(web.entry, source.join("ui/dist/index.html"));
    }
}
