//! macOS desktop integrations: tray, global shortcuts, LaunchAgent
//! autostart, URL-scheme deep links, native-messaging manifests, and
//! single-instance routing via a Unix lock + socket.

#![cfg(target_os = "macos")]

use std::{collections::HashMap, env, fs, path::PathBuf};

use global_hotkey::{
    GlobalHotKeyManager,
    hotkey::{Code, HotKey, Modifiers},
};
use sabine_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    Shortcut, TrayIcon,
};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuItem, PredefinedMenuItem},
};

pub(super) use super::{HotkeyRuntime, TrayRuntime};

pub(super) fn spawn_tray_icon(
    icon: &TrayIcon,
) -> Result<
    (
        TrayRuntime,
        HashMap<String, (String, String, Option<String>)>,
    ),
    String,
> {
    let menu = Menu::new();
    let mut actions = HashMap::new();
    for item in &icon.menu {
        if item.separator {
            menu.append(&PredefinedMenuItem::separator())
                .map_err(|error| error.to_string())?;
            continue;
        }
        let menu_item = MenuItem::new(item.label.clone(), item.enabled, None);
        actions.insert(
            menu_item.id().0.clone(),
            (icon.id.clone(), item.id.clone(), item.action.clone()),
        );
        menu.append(&menu_item).map_err(|error| error.to_string())?;
    }
    let tray_icon = load_tray_icon(icon)?;
    let tray = TrayIconBuilder::new()
        .with_tooltip(icon.tooltip.clone().unwrap_or_else(|| icon.title.clone()))
        .with_icon(tray_icon)
        .with_menu(Box::new(menu))
        .with_title(icon.title.clone())
        .build()
        .map_err(|error| error.to_string())?;
    Ok((TrayRuntime { _icon: tray }, actions))
}

pub(super) fn load_tray_icon(icon: &TrayIcon) -> Result<Icon, String> {
    if let Some(path) = &icon.icon_path
        && path.exists()
    {
        let image = image::open(path)
            .map_err(|error| error.to_string())?
            .into_rgba8();
        let (width, height) = image.dimensions();
        return Icon::from_rgba(image.into_raw(), width, height).map_err(|error| error.to_string());
    }
    let mut rgba = vec![0u8; 16 * 16 * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[0] = 0xE8;
        pixel[1] = 0xE8;
        pixel[2] = 0xE8;
        pixel[3] = 0xFF;
    }
    Icon::from_rgba(rgba, 16, 16).map_err(|error| error.to_string())
}

pub(super) fn spawn_global_shortcuts(
    registrations: &[GlobalShortcutRegistration],
) -> Result<(HotkeyRuntime, HashMap<u32, (String, String)>), String> {
    let manager = GlobalHotKeyManager::new().map_err(|error| error.to_string())?;
    let mut actions = HashMap::new();
    let mut keys = Vec::new();
    for registration in registrations {
        let hotkey = shortcut_to_hotkey(&registration.shortcut)?;
        manager
            .register(hotkey)
            .map_err(|error| error.to_string())?;
        actions.insert(
            hotkey.id(),
            (registration.id.clone(), registration.action.clone()),
        );
        keys.push(hotkey);
    }
    Ok((
        HotkeyRuntime {
            _manager: manager,
            _keys: keys,
        },
        actions,
    ))
}

pub(super) fn shortcut_to_hotkey(shortcut: &Shortcut) -> Result<HotKey, String> {
    let mut modifiers = Modifiers::empty();
    if shortcut.modifiers.ctrl {
        modifiers |= Modifiers::CONTROL;
    }
    if shortcut.modifiers.alt {
        modifiers |= Modifiers::ALT;
    }
    if shortcut.modifiers.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if shortcut.modifiers.meta {
        modifiers |= Modifiers::SUPER;
    }
    let code = parse_key_code(&shortcut.key)?;
    Ok(HotKey::new(Some(modifiers), code))
}

pub(super) fn parse_key_code(key: &str) -> Result<Code, String> {
    let normalized = key.trim().to_ascii_uppercase();
    match normalized.as_str() {
        "A" => Ok(Code::KeyA),
        "B" => Ok(Code::KeyB),
        "C" => Ok(Code::KeyC),
        "D" => Ok(Code::KeyD),
        "E" => Ok(Code::KeyE),
        "F" => Ok(Code::KeyF),
        "G" => Ok(Code::KeyG),
        "H" => Ok(Code::KeyH),
        "I" => Ok(Code::KeyI),
        "J" => Ok(Code::KeyJ),
        "K" => Ok(Code::KeyK),
        "L" => Ok(Code::KeyL),
        "M" => Ok(Code::KeyM),
        "N" => Ok(Code::KeyN),
        "O" => Ok(Code::KeyO),
        "P" => Ok(Code::KeyP),
        "Q" => Ok(Code::KeyQ),
        "R" => Ok(Code::KeyR),
        "S" => Ok(Code::KeyS),
        "T" => Ok(Code::KeyT),
        "U" => Ok(Code::KeyU),
        "V" => Ok(Code::KeyV),
        "W" => Ok(Code::KeyW),
        "X" => Ok(Code::KeyX),
        "Y" => Ok(Code::KeyY),
        "Z" => Ok(Code::KeyZ),
        "0" | "DIGIT0" => Ok(Code::Digit0),
        "1" | "DIGIT1" => Ok(Code::Digit1),
        "2" | "DIGIT2" => Ok(Code::Digit2),
        "3" | "DIGIT3" => Ok(Code::Digit3),
        "4" | "DIGIT4" => Ok(Code::Digit4),
        "5" | "DIGIT5" => Ok(Code::Digit5),
        "6" | "DIGIT6" => Ok(Code::Digit6),
        "7" | "DIGIT7" => Ok(Code::Digit7),
        "8" | "DIGIT8" => Ok(Code::Digit8),
        "9" | "DIGIT9" => Ok(Code::Digit9),
        "F1" => Ok(Code::F1),
        "F2" => Ok(Code::F2),
        "F3" => Ok(Code::F3),
        "F4" => Ok(Code::F4),
        "F5" => Ok(Code::F5),
        "F6" => Ok(Code::F6),
        "F7" => Ok(Code::F7),
        "F8" => Ok(Code::F8),
        "F9" => Ok(Code::F9),
        "F10" => Ok(Code::F10),
        "F11" => Ok(Code::F11),
        "F12" => Ok(Code::F12),
        "SPACE" => Ok(Code::Space),
        "ENTER" | "RETURN" => Ok(Code::Enter),
        "ESCAPE" | "ESC" => Ok(Code::Escape),
        "TAB" => Ok(Code::Tab),
        other => Err(format!("unsupported shortcut key: {other}")),
    }
}

pub(super) fn write_autostart_entry(entry: &AutostartEntry) -> Result<(), String> {
    let agents = home_dir()?.join("Library").join("LaunchAgents");
    fs::create_dir_all(&agents).map_err(|error| error.to_string())?;
    let label = format!("dev.sabine.{}", sanitize_id(&entry.id));
    let plist_path = agents.join(format!("{label}.plist"));
    if !entry.enabled {
        let _ = fs::remove_file(&plist_path);
        return Ok(());
    }
    let program_args = shell_words(&entry.command);
    let args_xml = program_args
        .iter()
        .map(|arg| format!("    <string>{}</string>", xml_escape(arg)))
        .collect::<Vec<_>>()
        .join("\n");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{args_xml}
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#
    );
    fs::write(plist_path, plist).map_err(|error| error.to_string())
}

pub(super) fn register_deep_links(registration: &DeepLinkRegistration) -> Result<(), String> {
    // Runtime URL-handler registration on macOS requires an app bundle
    // Info.plist. Persist a helper plist under Application Support so
    // packagers / CI can merge the schemes, and best-effort write a
    // defaults domain hint for development shells.
    let support = home_dir()?
        .join("Library")
        .join("Application Support")
        .join("sabine")
        .join("deep-links");
    fs::create_dir_all(&support).map_err(|error| error.to_string())?;
    let path = support.join(format!("{}.json", sanitize_id(&registration.id)));
    let payload = serde_json::json!({
        "id": registration.id,
        "schemes": registration.schemes,
        "executable": std::env::current_exe().ok(),
    });
    fs::write(
        path,
        serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn register_native_messaging_host(host: &NativeMessagingHost) -> Result<(), String> {
    let name = sanitize_id(&host.name.to_ascii_lowercase());
    let manifest_dir = home_dir()?
        .join("Library")
        .join("Application Support")
        .join("sabine")
        .join("native-messaging");
    fs::create_dir_all(&manifest_dir).map_err(|error| error.to_string())?;
    let manifest_path = manifest_dir.join(format!("{name}.json"));
    let executable = host
        .executable
        .canonicalize()
        .unwrap_or_else(|_| host.executable.clone());
    let manifest = serde_json::json!({
        "name": name,
        "description": host.id,
        "path": executable,
        "type": "stdio",
        "allowed_origins": host.allowed_origins,
    });
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    for browser in [
        "Google/Chrome/NativeMessagingHosts",
        "Chromium/NativeMessagingHosts",
        "Microsoft Edge/NativeMessagingHosts",
        "BraveSoftware/Brave-Browser/NativeMessagingHosts",
        "Mozilla/NativeMessagingHosts",
    ] {
        let dir = home_dir()?
            .join("Library")
            .join("Application Support")
            .join(browser);
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        fs::copy(&manifest_path, dir.join(format!("{name}.json")))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is required for macOS desktop integration".to_string())
}

pub(super) fn sanitize_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "app".to_string()
    } else {
        sanitized
    }
}

pub(super) fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in command.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    if words.is_empty() {
        words.push(command.to_string());
    }
    words
}

pub(super) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
