use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::util::data_home;

pub(crate) fn integrate_appimage(app_id: &str) -> Result<(), String> {
    let Some(appimage) = env::var_os("APPIMAGE").map(PathBuf::from) else {
        return Ok(());
    };
    let Some(appdir) = env::var_os("APPDIR").map(PathBuf::from) else {
        return Ok(());
    };
    let source_entry = appdir.join(format!("{app_id}.desktop"));
    if !source_entry.is_file() {
        return Ok(());
    }

    let data_home = data_home().map_err(|error| error.to_string())?;
    let applications = data_home.join("applications");
    fs::create_dir_all(&applications).map_err(|error| error.to_string())?;
    let entry = installed_desktop_entry(&source_entry, &appimage)?;
    let entry_changed = write_if_changed(&applications.join(format!("{app_id}.desktop")), &entry)?;

    let source_icons = appdir.join("usr/share/icons/hicolor");
    let icon_root = data_home.join("icons/hicolor");
    let icons_changed = copy_changed_tree(&source_icons, &icon_root)?;

    if entry_changed {
        let _ = Command::new("update-desktop-database")
            .arg(&applications)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    if icons_changed {
        let _ = Command::new("gtk-update-icon-cache")
            .args(["-q", "-t"])
            .arg(&icon_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

fn installed_desktop_entry(source: &Path, appimage: &Path) -> Result<Vec<u8>, String> {
    let body = fs::read_to_string(source).map_err(|error| error.to_string())?;
    let exec = desktop_exec(appimage);
    let mut replaced = false;
    let mut installed = String::with_capacity(body.len() + exec.len());
    for line in body.lines() {
        if line.starts_with("Exec=") {
            installed.push_str("Exec=");
            installed.push_str(&exec);
            replaced = true;
        } else {
            installed.push_str(line);
        }
        installed.push('\n');
    }
    if !replaced {
        return Err("AppImage desktop entry has no Exec field".to_string());
    }
    Ok(installed.into_bytes())
}

fn desktop_exec(path: &Path) -> String {
    let escaped = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!("\"{escaped}\" %U")
}

fn write_if_changed(path: &Path, contents: &[u8]) -> Result<bool, String> {
    if fs::read(path).is_ok_and(|existing| existing == contents) {
        return Ok(false);
    }
    fs::write(path, contents).map_err(|error| error.to_string())?;
    Ok(true)
}

fn copy_changed_tree(source: &Path, destination: &Path) -> Result<bool, String> {
    if !source.is_dir() {
        return Ok(false);
    }
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut changed = false;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            changed |= copy_changed_tree(&source_path, &destination_path)?;
        } else if source_path.is_file() {
            let contents = fs::read(&source_path).map_err(|error| error.to_string())?;
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            changed |= write_if_changed(&destination_path, &contents)?;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_appimage_exec_paths() {
        assert_eq!(
            desktop_exec(Path::new("/tmp/Limbo AppImage")),
            "\"/tmp/Limbo AppImage\" %U"
        );
    }
}
