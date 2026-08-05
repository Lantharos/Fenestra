use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct DevProject {
    pub cargo_manifest: PathBuf,
    pub frontend: Option<DevFrontend>,
}

#[derive(Debug, Clone)]
pub struct DevFrontend {
    pub root: PathBuf,
    pub port: u16,
    pub url: String,
    pub command: String,
    pub package_manager: &'static str,
}

#[derive(Debug, Default, Deserialize)]
struct MullionFile {
    #[serde(default)]
    app: AppSection,
    #[serde(default)]
    web: WebSection,
}

#[derive(Debug, Default, Deserialize)]
struct AppSection {
    cargo_manifest: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WebSection {
    root: Option<String>,
    dev_url: Option<String>,
    dev_port: Option<u16>,
    dev_command: Option<String>,
}

pub fn resolve_dev_project(source: &Path) -> Result<DevProject, String> {
    let source_dir = absolute_path(source)?;
    let mullion_path = source_dir.join("Mullion.toml");
    let mullion = if mullion_path.is_file() {
        Some(read_mullion_file(&mullion_path)?)
    } else {
        None
    };
    let cargo_manifest = mullion
        .as_ref()
        .and_then(|file| file.app.cargo_manifest.as_ref())
        .map(|path| source_dir.join(path))
        .unwrap_or_else(|| source_dir.join("Cargo.toml"));
    if !cargo_manifest.is_file() {
        return Err(format!(
            "no Cargo.toml found at {} (looked from {})",
            cargo_manifest.display(),
            source_dir.display()
        ));
    }
    let web = mullion.as_ref().map(|file| &file.web);
    let frontend = resolve_frontend(&source_dir, web)?;
    Ok(DevProject {
        cargo_manifest,
        frontend,
    })
}

fn resolve_frontend(
    source_dir: &Path,
    web: Option<&WebSection>,
) -> Result<Option<DevFrontend>, String> {
    let configured_root = web
        .and_then(|section| section.root.as_ref())
        .map(|root| source_dir.join(root));
    let root = configured_root
        .filter(|root| root.join("package.json").is_file())
        .or_else(|| detect_package_root(source_dir));
    let Some(root) = root else {
        return Ok(None);
    };
    let package_json = root.join("package.json");
    if !package_json.is_file() {
        return Ok(None);
    }
    let package = read_package_json(&package_json)?;
    let Some(script) = detect_dev_script(&package) else {
        return Ok(None);
    };
    let package_manager = package_manager(&root, &package);
    let port = web
        .and_then(|section| section.dev_port)
        .or_else(|| {
            web.and_then(|section| section.dev_url.as_deref())
                .and_then(parse_localhost_port)
        })
        .or_else(|| detect_vite_port(&root))
        .unwrap_or(5173);
    let url = web
        .and_then(|section| section.dev_url.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| vite_dev_url(port));
    let command = web
        .and_then(|section| section.dev_command.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{package_manager} run {script} -- --port {port} --strictPort"));
    Ok(Some(DevFrontend {
        root,
        port,
        url,
        command,
        package_manager,
    }))
}

pub fn detect_package_root(source_dir: &Path) -> Option<PathBuf> {
    ["ui", "frontend", "web", "."]
        .iter()
        .map(|candidate| source_dir.join(candidate))
        .find(|candidate| candidate.join("package.json").is_file())
}

pub fn detect_web_build_command(root: &Path) -> Option<String> {
    let package_json = root.join("package.json");
    if !package_json.is_file() {
        return None;
    }
    let text = fs::read_to_string(package_json).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    value
        .get("scripts")
        .and_then(|scripts| scripts.get("build"))
        .and_then(serde_json::Value::as_str)?;
    Some(format!("{} run build", package_manager(root, &value)))
}

pub fn package_manager(root: &Path, package: &serde_json::Value) -> &'static str {
    if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
        return "bun";
    }
    if root.join("pnpm-lock.yaml").exists() {
        return "pnpm";
    }
    if root.join("yarn.lock").exists() {
        return "yarn";
    }
    if package
        .get("packageManager")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value.starts_with("bun@"))
    {
        return "bun";
    }
    if command_exists("bun") { "bun" } else { "npm" }
}

pub fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|path| path.join(name).is_file()))
}

fn detect_dev_script(package: &serde_json::Value) -> Option<String> {
    let scripts = package.get("scripts")?;
    for name in ["dev", "start"] {
        if scripts
            .get(name)
            .and_then(serde_json::Value::as_str)
            .is_some()
        {
            return Some(name.to_string());
        }
    }
    None
}

fn detect_vite_port(root: &Path) -> Option<u16> {
    for name in [
        "vite.config.ts",
        "vite.config.js",
        "vite.config.mjs",
        "vite.config.cjs",
    ] {
        let path = root.join(name);
        if !path.is_file() {
            // Also check project root when package.json is in a parent with vite at source root
            continue;
        }
        let text = fs::read_to_string(path).ok()?;
        if let Some(port) = extract_port_from_vite_config(&text) {
            return Some(port);
        }
    }
    // Parent source dir configs (template puts vite.config.ts next to package.json at project root)
    for name in [
        "vite.config.ts",
        "vite.config.js",
        "vite.config.mjs",
        "vite.config.cjs",
    ] {
        if let Some(parent) = root.parent() {
            let path = parent.join(name);
            if path.is_file() {
                if let Ok(text) = fs::read_to_string(path) {
                    if let Some(port) = extract_port_from_vite_config(&text) {
                        return Some(port);
                    }
                }
            }
        }
    }
    None
}

fn extract_port_from_vite_config(text: &str) -> Option<u16> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("port:")
            .or_else(|| trimmed.strip_prefix("port :"))
        {
            let digits: String = rest
                .trim()
                .trim_matches(',')
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect();
            if let Ok(port) = digits.parse::<u16>() {
                return Some(port);
            }
        }
    }
    None
}

fn read_package_json(path: &Path) -> Result<serde_json::Value, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn read_mullion_file(path: &Path) -> Result<MullionFile, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    toml::from_str(&text).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| error.to_string())?
    };
    Ok(dunce_simplify(absolute))
}

fn dunce_simplify(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn vite_dev_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

fn parse_localhost_port(url: &str) -> Option<u16> {
    let url = url.trim();
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if let Some((host, port)) = host_port.rsplit_once(':') {
        if matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1") {
            return port.parse().ok();
        }
    }
    None
}
