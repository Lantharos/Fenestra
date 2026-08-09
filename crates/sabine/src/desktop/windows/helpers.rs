// Windows desktop integrations: tray, global shortcuts, autostart,
// deep links, native-messaging manifests, and single-instance routing.
// Events are queued as `PlatformEvent` values and drained by
// `SabineProcess::take_desktop_events` / the bridge forwarder.

#![cfg(target_os = "windows")]

use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use global_hotkey::{
    GlobalHotKeyManager,
    hotkey::{Code, HotKey, Modifiers},
};
use sabine_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, Shortcut, SingleInstanceActivation, SingleInstancePolicy, TrayIcon,
};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuItem, PredefinedMenuItem},
};
use windows::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE},
    System::{
        Registry::{
            HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
            RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
        },
        Threading::CreateMutexW,
    },
};

pub(super) use super::{EventQueue, HotkeyRuntime, TrayRuntime};

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
    let mut builder = TrayIconBuilder::new()
        .with_tooltip(icon.tooltip.clone().unwrap_or_else(|| icon.title.clone()))
        .with_icon(tray_icon)
        .with_menu(Box::new(menu));
    if !icon.title.is_empty() {
        builder = builder.with_title(icon.title.clone());
    }
    let tray = builder.build().map_err(|error| error.to_string())?;
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
    let name = sanitize_id(&entry.id);
    let key_path = format!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    if entry.enabled {
        set_registry_string(HKEY_CURRENT_USER, &key_path, &name, &entry.command)?;
    } else {
        let _ = delete_registry_value(HKEY_CURRENT_USER, &key_path, &name);
    }
    Ok(())
}

pub(super) fn register_deep_links(registration: &DeepLinkRegistration) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let command = format!("\"{}\" \"%1\"", exe.display());
    for scheme in &registration.schemes {
        let scheme = sanitize_scheme(scheme);
        let base = format!("Software\\Classes\\{scheme}");
        set_registry_string(HKEY_CURRENT_USER, &base, "", &format!("URL:{scheme}"))?;
        set_registry_string(HKEY_CURRENT_USER, &base, "URL Protocol", "")?;
        set_registry_string(
            HKEY_CURRENT_USER,
            &format!("{base}\\shell\\open\\command"),
            "",
            &command,
        )?;
    }
    Ok(())
}

pub(super) fn register_native_messaging_host(host: &NativeMessagingHost) -> Result<(), String> {
    let name = sanitize_native_host_name(&host.name);
    let manifest_dir = local_app_data()?.join("sabine").join("native-messaging");
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

    let manifest_str = manifest_path.display().to_string();
    for browser in [
        "Software\\Google\\Chrome\\NativeMessagingHosts",
        "Software\\Chromium\\NativeMessagingHosts",
        "Software\\Microsoft\\Edge\\NativeMessagingHosts",
        "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts",
    ] {
        let key = format!("{browser}\\{name}");
        set_registry_string(HKEY_CURRENT_USER, &key, "", &manifest_str)?;
    }

    let firefox_dir = roaming_app_data()?
        .join("Mozilla")
        .join("NativeMessagingHosts");
    fs::create_dir_all(&firefox_dir).map_err(|error| error.to_string())?;
    fs::copy(&manifest_path, firefox_dir.join(format!("{name}.json")))
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) struct SingleInstanceGuard {
    mutex: HANDLE,
    _listener: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    wake_port: u16,
}

impl SingleInstanceGuard {
    pub(super) fn acquire(
        id: Option<&str>,
        policy: SingleInstancePolicy,
        events: EventQueue,
    ) -> Result<Self, String> {
        let name = format!(
            "Local\\sabine-{}",
            sanitize_id(id.unwrap_or("default-instance"))
        );
        let wide = wide_null(&name);
        let handle = unsafe { CreateMutexW(None, false, windows::core::PCWSTR(wide.as_ptr())) }
            .map_err(|error| error.to_string())?;
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already {
            unsafe {
                let _ = CloseHandle(handle);
            }
            notify_existing_instance(&name)?;
            return Err("another Sabine instance is already running".to_string());
        }
        let running = Arc::new(AtomicBool::new(true));
        let (listener, wake_port) =
            match spawn_instance_listener(name, policy, events, Arc::clone(&running)) {
                Ok(runtime) => runtime,
                Err(error) => {
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                    return Err(error);
                }
            };
        Ok(Self {
            mutex: handle,
            _listener: Some(listener),
            running,
            wake_port,
        })
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.wake_port));
        if let Some(thread) = self._listener.take() {
            let _ = thread.join();
        }
        unsafe {
            let _ = CloseHandle(self.mutex);
        }
    }
}

pub(super) fn spawn_instance_listener(
    name: String,
    policy: SingleInstancePolicy,
    events: EventQueue,
    running: Arc<AtomicBool>,
) -> Result<(JoinHandle<()>, u16), String> {
    let port_path = instance_port_path(&name)?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    fs::write(&port_path, port.to_string()).map_err(|error| error.to_string())?;
    let thread = thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }
                    let mut buffer = String::new();
                    let _ = stream.read_to_string(&mut buffer);
                    let arguments = buffer
                        .lines()
                        .map(str::to_string)
                        .filter(|line| !line.is_empty())
                        .collect::<Vec<_>>();
                    push_event(
                        &events,
                        PlatformEvent::SingleInstance(SingleInstanceActivation::new(
                            policy, arguments,
                        )),
                    );
                }
                Err(_) => break,
            }
        }
        let _ = fs::remove_file(port_path);
    });
    Ok((thread, port))
}

pub(super) fn notify_existing_instance(name: &str) -> Result<(), String> {
    let port_path = instance_port_path(name)?;
    let port = fs::read_to_string(port_path)
        .map_err(|error| error.to_string())?
        .trim()
        .parse::<u16>()
        .map_err(|error| error.to_string())?;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| error.to_string())?;
    let payload = std::env::args().collect::<Vec<_>>().join("\n");
    stream
        .write_all(payload.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn instance_port_path(name: &str) -> Result<PathBuf, String> {
    let dir = local_app_data()?.join("sabine").join("instances");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(format!("{}.port", sanitize_id(name))))
}

pub(super) fn set_registry_string(
    root: windows::Win32::System::Registry::HKEY,
    subkey: &str,
    value_name: &str,
    data: &str,
) -> Result<(), String> {
    let subkey_wide = wide_null(subkey);
    let mut key = windows::Win32::System::Registry::HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            root,
            windows::core::PCWSTR(subkey_wide.as_ptr()),
            Some(0),
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("RegCreateKeyExW failed: {status:?}"));
    }
    let value_wide = wide_null(value_name);
    let data_wide = wide_null(data);
    let bytes =
        unsafe { std::slice::from_raw_parts(data_wide.as_ptr() as *const u8, data_wide.len() * 2) };
    let result = unsafe {
        RegSetValueExW(
            key,
            windows::core::PCWSTR(value_wide.as_ptr()),
            Some(0),
            REG_SZ,
            Some(bytes),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if result != ERROR_SUCCESS {
        return Err(format!("RegSetValueExW failed: {result:?}"));
    }
    Ok(())
}

pub(super) fn delete_registry_value(
    root: windows::Win32::System::Registry::HKEY,
    subkey: &str,
    value_name: &str,
) -> Result<(), String> {
    let path = format!("{subkey}\\{value_name}");
    let wide = wide_null(&path);
    let _ = unsafe { RegDeleteTreeW(root, windows::core::PCWSTR(wide.as_ptr())) };
    Ok(())
}

pub(super) fn push_event(events: &EventQueue, event: PlatformEvent) {
    let _ = events.send(event);
}

pub(super) fn local_app_data() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA is required".to_string())
}

pub(super) fn roaming_app_data() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "APPDATA is required".to_string())
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

pub(super) fn sanitize_scheme(value: &str) -> String {
    sanitize_id(&value.to_ascii_lowercase())
}

pub(super) fn sanitize_native_host_name(value: &str) -> String {
    sanitize_id(&value.to_ascii_lowercase())
}

pub(super) fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
