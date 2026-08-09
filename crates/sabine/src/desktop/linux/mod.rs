use std::{
    collections::BTreeMap,
    thread::{self, JoinHandle},
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

pub(super) type EventQueue = crossbeam_channel::Sender<PlatformEvent>;

use instance::SingleInstanceGuard;
use links::{register_deep_links, register_native_messaging_host, write_autostart_entry};
use shortcuts::{ShortcutRuntime, spawn_global_shortcut};
use tray::{TrayRuntime, spawn_tray_icon};

pub struct DesktopServiceState {
    event_sender: EventQueue,
    event_receiver: crossbeam_channel::Receiver<PlatformEvent>,
    tray: Option<TrayRuntime>,
    shortcuts: BTreeMap<String, ShortcutRuntime>,
    single_instance: Option<SingleInstanceGuard>,
}

impl std::fmt::Debug for DesktopServiceState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DesktopServiceState")
            .field("queued_events", &self.event_receiver.len())
            .field("shortcuts", &self.shortcuts.keys().collect::<Vec<_>>())
            .field("single_instance", &self.single_instance.is_some())
            .finish()
    }
}

impl DesktopServiceState {
    fn new() -> Self {
        let (event_sender, event_receiver) = crossbeam_channel::unbounded();
        Self {
            event_sender,
            event_receiver,
            tray: None,
            shortcuts: BTreeMap::new(),
            single_instance: None,
        }
    }

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
    let mut state = DesktopServiceState::new();
    if let Some(policy) = single_instance_policy
        && policy != SingleInstancePolicy::AllowMultiple
    {
        state.single_instance = Some(SingleInstanceGuard::acquire(
            single_instance_id,
            policy,
            state.event_sender.clone(),
        )?);
    }
    for entry in autostart {
        write_autostart_entry(entry).map_err(|error| error.to_string())?;
    }
    if let Some(icon) = tray_icon {
        state.tray = Some(spawn_tray_icon(icon, state.event_sender.clone())?);
    }
    for registration in global_shortcuts {
        state.shortcuts.insert(
            registration.id.clone(),
            spawn_global_shortcut(registration, state.event_sender.clone()),
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
    stop: crossbeam_channel::Receiver<()>,
    mut emit: impl FnMut(PlatformEvent) + Send + 'static,
) -> JoinHandle<()> {
    let events = services.event_receiver.clone();
    thread::spawn(move || {
        loop {
            crossbeam_channel::select! {
                recv(stop) -> _ => break,
                recv(events) -> event => match event {
                    Ok(event) => emit(event),
                    Err(_) => break,
                },
            }
        }
    })
}
