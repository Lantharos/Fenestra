// Windows desktop integrations: tray, global shortcuts, autostart,
// deep links, native-messaging manifests, and single-instance routing.
// Events are queued as `PlatformEvent` values and drained by
// `SabineProcess::take_desktop_events` / the bridge forwarder.

#![cfg(target_os = "windows")]

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use sabine_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutActivation, GlobalShortcutRegistration,
    NativeMessagingHost, PlatformEvent, SingleInstancePolicy, TrayActivation, TrayIcon,
};
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent, menu::MenuEvent};

pub(super) type EventQueue = Arc<Mutex<Vec<PlatformEvent>>>;

mod helpers;
use helpers::*;

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
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Ok(actions) = self.menu_actions.lock()
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
    let menu_actions = Arc::clone(&state.menu_actions);
    let shortcut_actions = Arc::clone(&state.shortcut_actions);
    let tray_id = state.tray_id.clone();
    thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            if let Ok(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) = TrayIconEvent::receiver().try_recv()
                && let Some(tray_id) = &tray_id
            {
                push_event(
                    &events,
                    PlatformEvent::Tray(TrayActivation::new(tray_id.clone())),
                );
            }
            if let Ok(event) = MenuEvent::receiver().try_recv()
                && let Ok(actions) = menu_actions.lock()
                && let Some((tray_id, item_id, action)) = actions.get(&event.id.0)
            {
                push_event(
                    &events,
                    PlatformEvent::Tray(TrayActivation::item(
                        tray_id.clone(),
                        item_id.clone(),
                        action.clone(),
                    )),
                );
            }
            if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv()
                && event.state == HotKeyState::Pressed
                && let Ok(actions) = shortcut_actions.lock()
                && let Some((id, action)) = actions.get(&event.id())
            {
                push_event(
                    &events,
                    PlatformEvent::GlobalShortcut(GlobalShortcutActivation::new(
                        id.clone(),
                        action.clone(),
                    )),
                );
            }
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

pub(super) struct TrayRuntime {
    _icon: tray_icon::TrayIcon,
}

pub(super) struct HotkeyRuntime {
    _manager: GlobalHotKeyManager,
    _keys: Vec<HotKey>,
}
