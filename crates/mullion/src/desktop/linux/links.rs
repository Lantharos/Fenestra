use std::{collections::BTreeSet, fs, io, path::PathBuf};

use mullion_platform::{AutostartEntry, DeepLinkRegistration, NativeMessagingHost};

use super::util::*;

pub(super) fn write_autostart_entry(entry: &AutostartEntry) -> io::Result<()> {
    let path = config_home()?
        .join("autostart")
        .join(format!("{}.desktop", sanitize_desktop_id(&entry.id)));
    if !entry.enabled {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    write_file(path, &desktop_entry(&entry.id, &entry.name, &entry.command))
}

pub(super) fn register_deep_links(registration: &DeepLinkRegistration) -> io::Result<()> {
    let desktop_id = format!("{}.desktop", sanitize_desktop_id(&registration.id));
    let path = config_home()?.join("mimeapps.list");
    let mut content = fs::read_to_string(&path).unwrap_or_default();
    for scheme in &registration.schemes {
        let scheme = sanitize_scheme(scheme);
        if !scheme.is_empty() {
            content = set_mime_default(&content, &scheme, &desktop_id);
        }
    }
    write_file(path, &content)
}

pub(super) fn register_native_messaging_host(host: &NativeMessagingHost) -> io::Result<()> {
    let name = sanitize_native_host_name(&host.id);
    let chrome_manifest = native_messaging_manifest(host, &name, "allowed_origins");
    for browser in ["google-chrome", "chromium", "BraveSoftware/Brave-Browser"] {
        write_file(
            config_home()?
                .join(browser)
                .join("NativeMessagingHosts")
                .join(format!("{name}.json")),
            &chrome_manifest,
        )?;
    }
    let firefox_manifest = native_messaging_manifest(host, &name, "allowed_extensions");
    write_file(
        home_dir()?
            .join(".mozilla/native-messaging-hosts")
            .join(format!("{name}.json")),
        &firefox_manifest,
    )
}

pub(super) fn write_file(path: PathBuf, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

pub(super) fn desktop_entry(id: &str, name: &str, command: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nGenericName={}\nComment={}\nExec={}\nIcon={}\nTerminal=false\nNoDisplay=true\nStartupNotify=false\nCategories=Utility;\n",
        desktop_value(name),
        desktop_value(name),
        desktop_value(name),
        desktop_value(command),
        desktop_value(id)
    )
}

pub(super) fn set_mime_default(content: &str, scheme: &str, desktop_id: &str) -> String {
    let key = format!("x-scheme-handler/{scheme}");
    let value = format!("{key}={desktop_id}");
    let mut lines = content.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let Some(section_start) = lines
        .iter()
        .position(|line| line.trim() == "[Default Applications]")
    else {
        if !lines.is_empty() && lines.last().is_some_and(|line| !line.is_empty()) {
            lines.push(String::new());
        }
        lines.push("[Default Applications]".to_string());
        lines.push(value);
        return finish_lines(lines);
    };
    let section_end = lines
        .iter()
        .enumerate()
        .skip(section_start + 1)
        .find_map(|(index, line)| line.trim().starts_with('[').then_some(index))
        .unwrap_or(lines.len());
    if let Some(index) = lines[section_start + 1..section_end]
        .iter()
        .position(|line| {
            line.split_once('=')
                .is_some_and(|(line_key, _)| line_key == key)
        })
    {
        lines[section_start + 1 + index] = value;
    } else {
        lines.insert(section_end, value);
    }
    finish_lines(lines)
}

pub(super) fn native_messaging_manifest(
    host: &NativeMessagingHost,
    name: &str,
    allowed_key: &str,
) -> String {
    let allowed = host
        .allowed_origins
        .iter()
        .map(|origin| format!("\"{}\"", json_value(origin)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    format!(
        "{{\n  \"name\": \"{}\",\n  \"description\": \"{}\",\n  \"path\": \"{}\",\n  \"type\": \"stdio\",\n  \"{}\": [{}]\n}}\n",
        json_value(name),
        json_value(&host.name),
        json_value(&host.executable.display().to_string()),
        allowed_key,
        allowed.join(", ")
    )
}

pub(super) fn finish_lines(lines: Vec<String>) -> String {
    let mut output = lines.join("\n");
    output.push('\n');
    output
}
