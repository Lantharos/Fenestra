//! macOS desktop integrations: tray, global shortcuts, LaunchAgent
//! autostart, URL-scheme deep links, native-messaging manifests, and
//! single-instance routing via a Unix lock + socket.

#![cfg(target_os = "macos")]

use std::{
    collections::HashMap,
    env, fs, io,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use mullion_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutActivation, GlobalShortcutRegistration,
    NativeMessagingHost, PlatformEvent, Shortcut, SingleInstanceActivation, SingleInstancePolicy,
    TrayActivation, TrayIcon,
};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

type EventQueue = Arc<Mutex<Vec<PlatformEvent>>>;

pub struct DesktopServiceState {
    events: EventQueue,
    _tray: Option<TrayRuntime>,
    _hotkeys: Option<HotkeyRuntime>,
    _single_instance: Option<SingleInstanceGuard>,
    menu_actions: Arc<Mutex<HashMap<String, (String, String, Option<String>)>>>,
    shortcut_actions: Arc<Mutex<HashMap<u32, (String, String)>>>,
    tray_id: Option<String>,
}

impl std::fmt::Debug for DesktopServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopServiceState")
            .field(
                "queued_events",
                &self.events.lock().map(|events| events.len()).ok(),
            )
            .finish()
    }
}

impl DesktopServiceState {
    pub fn take_events(&self) -> Vec<PlatformEvent> {
        self.events
            .lock()
            .map(|mut events| events.drain(..).collect())
            .unwrap_or_default()
    }

    pub fn poll_native_events(&self) {
        if let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    if let Some(tray_id) = &self.tray_id {
                        push_event(
                            &self.events,
                            PlatformEvent::Tray(TrayActivation::new(tray_id.clone())),
                        );
                    }
                }
                _ => {}
            }
        }
        if let Ok(event) = MenuEvent::receiver().try_recv()
            && let Ok(actions) = self.menu_actions.lock()
            && let Some((tray_id, item_id, action)) = actions.get(&event.id.0)
        {
            push_event(
                &self.events,
                PlatformEvent::Tray(TrayActivation::item(
                    tray_id.clone(),
                    item_id.clone(),
                    action.clone(),
                )),
            );
        }
        if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv()
            && event.state == HotKeyState::Pressed
            && let Ok(actions) = self.shortcut_actions.lock()
            && let Some((id, action)) = actions.get(&event.id())
        {
            push_event(
                &self.events,
                PlatformEvent::GlobalShortcut(GlobalShortcutActivation::new(
                    id.clone(),
                    action.clone(),
                )),
            );
        }
    }
}

pub fn apply_desktop_services(
    tray_icon: Option<&TrayIcon>,
    autostart: &[AutostartEntry],
    global_shortcuts: &[GlobalShortcutRegistration],
    deep_links: &[DeepLinkRegistration],
    native_messaging_hosts: &[NativeMessagingHost],
    single_instance_id: Option<&str>,
    single_instance_policy: Option<SingleInstancePolicy>,
) -> Result<DesktopServiceState, String> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut state = DesktopServiceState {
        events: Arc::clone(&events),
        _tray: None,
        _hotkeys: None,
        _single_instance: None,
        menu_actions: Arc::new(Mutex::new(HashMap::new())),
        shortcut_actions: Arc::new(Mutex::new(HashMap::new())),
        tray_id: tray_icon.map(|icon| icon.id.clone()),
    };

    if let Some(policy) = single_instance_policy
        && policy != SingleInstancePolicy::AllowMultiple
    {
        state._single_instance = Some(SingleInstanceGuard::acquire(
            single_instance_id,
            policy,
            Arc::clone(&events),
        )?);
    }

    for entry in autostart {
        write_autostart_entry(entry)?;
    }
    for registration in deep_links {
        register_deep_links(registration)?;
    }
    for host in native_messaging_hosts {
        register_native_messaging_host(host)?;
    }

    if let Some(icon) = tray_icon {
        let (tray, actions) = spawn_tray_icon(icon)?;
        *state.menu_actions.lock().unwrap() = actions;
        state._tray = Some(tray);
    }
    if !global_shortcuts.is_empty() {
        let (hotkeys, actions) = spawn_global_shortcuts(global_shortcuts)?;
        *state.shortcut_actions.lock().unwrap() = actions;
        state._hotkeys = Some(hotkeys);
    }

    Ok(state)
}

pub fn start_desktop_event_forwarder<F>(
    state: &DesktopServiceState,
    running: Arc<AtomicBool>,
    mut forwarder: F,
) -> JoinHandle<()>
where
    F: FnMut(PlatformEvent) + Send + 'static,
{
    let events = Arc::clone(&state.events);
    let services = MacosPollHandle {
        menu_actions: Arc::clone(&state.menu_actions),
        shortcut_actions: Arc::clone(&state.shortcut_actions),
        tray_id: state.tray_id.clone(),
        events: Arc::clone(&state.events),
    };
    thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            services.poll();
            let batch = events
                .lock()
                .map(|mut events| events.drain(..).collect::<Vec<_>>())
                .unwrap_or_default();
            for event in batch {
                forwarder(event);
            }
            thread::sleep(Duration::from_millis(16));
        }
    })
}

struct MacosPollHandle {
    menu_actions: Arc<Mutex<HashMap<String, (String, String, Option<String>)>>>,
    shortcut_actions: Arc<Mutex<HashMap<u32, (String, String)>>>,
    tray_id: Option<String>,
    events: EventQueue,
}

impl MacosPollHandle {
    fn poll(&self) {
        if let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    if let Some(tray_id) = &self.tray_id {
                        push_event(
                            &self.events,
                            PlatformEvent::Tray(TrayActivation::new(tray_id.clone())),
                        );
                    }
                }
                _ => {}
            }
        }
        if let Ok(event) = MenuEvent::receiver().try_recv()
            && let Ok(actions) = self.menu_actions.lock()
            && let Some((tray_id, item_id, action)) = actions.get(&event.id.0)
        {
            push_event(
                &self.events,
                PlatformEvent::Tray(TrayActivation::item(
                    tray_id.clone(),
                    item_id.clone(),
                    action.clone(),
                )),
            );
        }
        if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv()
            && event.state == HotKeyState::Pressed
            && let Ok(actions) = self.shortcut_actions.lock()
            && let Some((id, action)) = actions.get(&event.id())
        {
            push_event(
                &self.events,
                PlatformEvent::GlobalShortcut(GlobalShortcutActivation::new(
                    id.clone(),
                    action.clone(),
                )),
            );
        }
    }
}

struct TrayRuntime {
    _icon: tray_icon::TrayIcon,
}

struct HotkeyRuntime {
    _manager: GlobalHotKeyManager,
    _keys: Vec<HotKey>,
}

fn spawn_tray_icon(
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

fn load_tray_icon(icon: &TrayIcon) -> Result<Icon, String> {
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

fn spawn_global_shortcuts(
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

fn shortcut_to_hotkey(shortcut: &Shortcut) -> Result<HotKey, String> {
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

fn parse_key_code(key: &str) -> Result<Code, String> {
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

fn write_autostart_entry(entry: &AutostartEntry) -> Result<(), String> {
    let agents = home_dir()?.join("Library").join("LaunchAgents");
    fs::create_dir_all(&agents).map_err(|error| error.to_string())?;
    let label = format!("dev.mullion.{}", sanitize_id(&entry.id));
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

fn register_deep_links(registration: &DeepLinkRegistration) -> Result<(), String> {
    // Runtime URL-handler registration on macOS requires an app bundle
    // Info.plist. Persist a helper plist under Application Support so
    // packagers / CI can merge the schemes, and best-effort write a
    // defaults domain hint for development shells.
    let support = home_dir()?
        .join("Library")
        .join("Application Support")
        .join("mullion")
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

fn register_native_messaging_host(host: &NativeMessagingHost) -> Result<(), String> {
    let name = sanitize_id(&host.name.to_ascii_lowercase());
    let manifest_dir = home_dir()?
        .join("Library")
        .join("Application Support")
        .join("mullion")
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

struct SingleInstanceGuard {
    _listener: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    lock_path: PathBuf,
    socket_path: PathBuf,
}

impl SingleInstanceGuard {
    fn acquire(
        id: Option<&str>,
        policy: SingleInstancePolicy,
        events: EventQueue,
    ) -> Result<Self, String> {
        let runtime = runtime_dir()?;
        fs::create_dir_all(&runtime).map_err(|error| error.to_string())?;
        let key = sanitize_id(id.unwrap_or("default-instance"));
        let lock_path = runtime.join(format!("{key}.lock"));
        let socket_path = runtime.join(format!("{key}.sock"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", std::process::id());
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                notify_existing_instance(&socket_path)?;
                return Err("another Mullion instance is already running".to_string());
            }
            Err(error) => return Err(error.to_string()),
        }
        let _ = fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let thread = thread::spawn(move || {
            while thread_running.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
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
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            _listener: Some(thread),
            running,
            lock_path,
            socket_path,
        })
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self._listener.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.lock_path);
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn notify_existing_instance(socket_path: &Path) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| error.to_string())?;
    let payload = std::env::args().collect::<Vec<_>>().join("\n");
    stream
        .write_all(payload.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn push_event(events: &EventQueue, event: PlatformEvent) {
    if let Ok(mut queue) = events.lock() {
        queue.push(event);
    }
}

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is required for macOS desktop integration".to_string())
}

fn runtime_dir() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(path).join("mullion"));
    }
    Ok(home_dir()?.join("Library").join("Caches").join("mullion"))
}

fn sanitize_id(value: &str) -> String {
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

fn shell_words(command: &str) -> Vec<String> {
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

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
