use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use sabine_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, SingleInstancePolicy, TrayIcon,
};

mod instance;
mod links;
mod shortcuts;
mod tray;
mod util;

pub(super) type EventQueue = std::sync::Arc<std::sync::Mutex<Vec<sabine_platform::PlatformEvent>>>;

use instance::SingleInstanceGuard;
use links::{register_deep_links, register_native_messaging_host, write_autostart_entry};
use shortcuts::{ShortcutRuntime, spawn_global_shortcut};
use tray::{TrayRuntime, spawn_tray_icon};

pub struct DesktopServiceState {
    events: EventQueue,
    tray: Option<TrayRuntime>,
    shortcuts: BTreeMap<String, ShortcutRuntime>,
    single_instance: Option<SingleInstanceGuard>,
}

impl std::fmt::Debug for DesktopServiceState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DesktopServiceState")
            .field(
                "queued_events",
                &self.events.lock().map(|events| events.len()).ok(),
            )
            .field("shortcuts", &self.shortcuts.keys().collect::<Vec<_>>())
            .field("single_instance", &self.single_instance.is_some())
            .finish()
    }
}

impl DesktopServiceState {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            tray: None,
            shortcuts: BTreeMap::new(),
            single_instance: None,
        }
    }

    pub fn take_events(&self) -> Vec<PlatformEvent> {
        self.events
            .lock()
            .map(|mut events| events.drain(..).collect())
            .unwrap_or_default()
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
    let mut state = DesktopServiceState::new();
    if let Some(policy) = single_instance_policy
        && policy != SingleInstancePolicy::AllowMultiple
    {
        state.single_instance = Some(SingleInstanceGuard::acquire(
            single_instance_id,
            policy,
            Arc::clone(&state.events),
        )?);
    }
    for entry in autostart {
        write_autostart_entry(entry).map_err(|error| error.to_string())?;
    }
    if let Some(icon) = tray_icon {
        state.tray = Some(spawn_tray_icon(icon, Arc::clone(&state.events))?);
    }
    for registration in global_shortcuts {
        state.shortcuts.insert(
            registration.id.clone(),
            spawn_global_shortcut(registration, Arc::clone(&state.events)),
        );
    }
    for registration in deep_links {
        register_deep_links(registration).map_err(|error| error.to_string())?;
    }
    for host in native_messaging_hosts {
        register_native_messaging_host(host).map_err(|error| error.to_string())?;
    }
    Ok(state)
}

pub fn start_desktop_event_forwarder(
    services: &DesktopServiceState,
    running: Arc<AtomicBool>,
    mut emit: impl FnMut(PlatformEvent) + Send + 'static,
) -> JoinHandle<()> {
    let events = Arc::clone(&services.events);
    thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            let drained = events
                .lock()
                .map(|mut events| events.drain(..).collect::<Vec<_>>())
                .unwrap_or_default();
            for event in drained {
                emit(event);
            }
            thread::sleep(Duration::from_millis(8));
        }
    })
}
