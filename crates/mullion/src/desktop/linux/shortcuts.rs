use std::thread::{self, JoinHandle};

use futures_util::StreamExt;
use mullion_platform::{GlobalShortcutActivation, GlobalShortcutRegistration, PlatformEvent};

use super::EventQueue;
use super::links::{desktop_entry, write_file};
use super::util::{data_home, sanitize_desktop_id};

pub(super) fn spawn_global_shortcut(
    registration: &GlobalShortcutRegistration,
    events: EventQueue,
) -> ShortcutRuntime {
    let registration = registration.clone();
    let thread = thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let _ = runtime.block_on(run_portal_shortcut(registration, events));
    });
    ShortcutRuntime { thread }
}
pub(super) struct ShortcutRuntime {
    thread: JoinHandle<()>,
}

impl Drop for ShortcutRuntime {
    fn drop(&mut self) {
        let _ = self.thread.thread().id();
    }
}
pub(super) async fn run_portal_shortcut(
    registration: GlobalShortcutRegistration,
    events: EventQueue,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use ashpd::desktop::{
        CreateSessionOptions,
        global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
    };

    ensure_portal_app_registration(&registration).await?;

    let Some(trigger) = portal_trigger_for_shortcut(&registration) else {
        return Err("global shortcut is not supported by the Linux portal backend".into());
    };
    let portal = GlobalShortcuts::new().await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    let mut activations = portal.receive_activated().await?;
    let description = registration
        .description
        .as_deref()
        .unwrap_or(&registration.action);
    let shortcut = NewShortcut::new(registration.id.as_str(), description)
        .preferred_trigger(Some(trigger.as_str()));
    let request = portal
        .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
        .await?;
    let response = request.response()?;
    if !response
        .shortcuts()
        .iter()
        .any(|shortcut| shortcut.id() == registration.id)
    {
        return Err("the portal did not bind the requested shortcut".into());
    }

    while let Some(event) = activations.next().await {
        if event.shortcut_id() != registration.id {
            continue;
        }
        let mut activation =
            GlobalShortcutActivation::new(registration.id.clone(), registration.action.clone());
        if let Some(token) = activation_token_from_options(event.options()) {
            activation = activation.activation_token(token);
        }
        if let Ok(mut events) = events.lock() {
            events.push(PlatformEvent::GlobalShortcut(activation));
        }
    }
    Ok(())
}

pub(super) fn portal_trigger_for_shortcut(
    registration: &GlobalShortcutRegistration,
) -> Option<String> {
    let shortcut = &registration.shortcut;
    let mut parts = Vec::new();
    if shortcut.modifiers.ctrl {
        parts.push("CTRL".to_string());
    }
    if shortcut.modifiers.alt {
        parts.push("ALT".to_string());
    }
    if shortcut.modifiers.shift {
        parts.push("SHIFT".to_string());
    }
    if shortcut.modifiers.meta {
        parts.push("LOGO".to_string());
    }
    let mut key = shortcut.key.trim().to_string();
    if key.is_empty() {
        return None;
    }
    if key.len() == 1 && key.is_ascii() {
        key.make_ascii_lowercase();
    }
    parts.push(key);
    Some(parts.join("+"))
}

pub(super) fn activation_token_from_options(
    options: &std::collections::HashMap<String, ashpd::zvariant::OwnedValue>,
) -> Option<String> {
    let value = options.get("activation_token")?.try_clone().ok()?;
    String::try_from(value)
        .ok()
        .filter(|token| !token.trim().is_empty())
}

pub(super) async fn ensure_portal_app_registration(
    registration: &GlobalShortcutRegistration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(app_id) = registration.app_id.as_deref() else {
        return Ok(());
    };
    if let Some(command) = registration.desktop_command.as_deref() {
        let app_name = registration
            .app_name
            .as_deref()
            .or(registration.description.as_deref())
            .unwrap_or(app_id);
        write_file(
            data_home()?
                .join("applications")
                .join(format!("{}.desktop", sanitize_desktop_id(app_id))),
            &desktop_entry(app_id, app_name, command),
        )?;
    }
    let app_id = ashpd::AppID::try_from(app_id)?;
    match ashpd::register_host_app(app_id).await {
        Ok(()) => Ok(()),
        Err(error) if portal_app_already_registered(&error) => Ok(()),
        Err(error) => Err(Box::new(error)),
    }
}

pub(super) fn portal_app_already_registered(error: &ashpd::Error) -> bool {
    let message = error.to_string();
    message.contains("already associated") || message.contains("already registered")
}
