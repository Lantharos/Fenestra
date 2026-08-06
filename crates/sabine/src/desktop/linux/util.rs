use std::{env, io, path::PathBuf};

pub(super) fn desktop_value(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

pub(super) fn json_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub(super) fn sanitize_desktop_id(value: &str) -> String {
    sanitize_with(value, |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_')
    })
}

pub(super) fn sanitize_scheme(value: &str) -> String {
    sanitize_with(&value.to_ascii_lowercase(), |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-')
    })
}

pub(super) fn sanitize_native_host_name(value: &str) -> String {
    sanitize_with(&value.to_ascii_lowercase(), |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.')
    })
}

pub(super) fn sanitize_with(value: &str, valid: impl Fn(char) -> bool) -> String {
    let sanitized = value
        .chars()
        .map(|ch| if valid(ch) { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "app".to_string()
    } else {
        sanitized
    }
}

pub(super) fn config_home() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".config"))
}

pub(super) fn data_home() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".local/share"))
}

pub(super) fn home_dir() -> io::Result<PathBuf> {
    env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME is required for Linux desktop integration",
        )
    })
}
