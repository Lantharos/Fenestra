//! macOS desktop integrations: tray, global shortcuts, LaunchAgent
//! autostart, URL-scheme deep links, native-messaging manifests, and
//! single-instance routing via a Unix lock + socket.

#![cfg(target_os = "macos")]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use sabine_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutActivation, GlobalShortcutRegistration,
    NativeMessagingHost, PlatformEvent, SingleInstancePolicy, TrayActivation, TrayIcon,
};
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent, menu::MenuEvent};

pub(super) type EventQueue = crossbeam_channel::Sender<PlatformEvent>;

mod helpers;
use helpers::*;

pub struct DesktopServiceState {
    _event_sender: EventQueue,
    event_receiver: crossbeam_channel::Receiver<PlatformEvent>,
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
            .field("queued_events", &self.event_receiver.len())
            .finish()
    }
}

impl DesktopServiceState {
    pub fn take_events(&self) -> Vec<PlatformEvent> {
        self.event_receiver.try_iter().collect()
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
    let (event_sender, event_receiver) = crossbeam_channel::unbounded();
    let mut state = DesktopServiceState {
        _event_sender: event_sender.clone(),
        event_receiver,
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
            event_sender.clone(),
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
    stop: crossbeam_channel::Receiver<()>,
    mut forwarder: F,
) -> JoinHandle<()>
where
    F: FnMut(PlatformEvent) + Send + 'static,
{
    let events = state.event_receiver.clone();
    let menu_actions = Arc::clone(&state.menu_actions);
    let shortcut_actions = Arc::clone(&state.shortcut_actions);
    let tray_id = state.tray_id.clone();
    thread::spawn(move || {
        loop {
            crossbeam_channel::select! {
                recv(stop) -> _ => break,
                recv(events) -> event => match event {
                    Ok(event) => forwarder(event),
                    Err(_) => break,
                },
                recv(TrayIconEvent::receiver()) -> event => {
                    if let Ok(TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }) = event
                        && let Some(tray_id) = &tray_id
                    {
                        forwarder(PlatformEvent::Tray(TrayActivation::new(tray_id.clone())));
                    }
                },
                recv(MenuEvent::receiver()) -> event => {
                    if let Ok(event) = event
                        && let Ok(actions) = menu_actions.lock()
                        && let Some((tray_id, item_id, action)) = actions.get(&event.id.0)
                    {
                        forwarder(PlatformEvent::Tray(TrayActivation::item(
                            tray_id.clone(),
                            item_id.clone(),
                            action.clone(),
                        )));
                    }
                },
                recv(GlobalHotKeyEvent::receiver()) -> event => {
                    if let Ok(event) = event
                        && event.state == HotKeyState::Pressed
                        && let Ok(actions) = shortcut_actions.lock()
                        && let Some((id, action)) = actions.get(&event.id())
                    {
                        forwarder(PlatformEvent::GlobalShortcut(
                            GlobalShortcutActivation::new(id.clone(), action.clone()),
                        ));
                    }
                },
            }
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
