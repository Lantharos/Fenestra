// Bridge command routing for guest webviews.
//
// `fenestra.guest.*` and the legacy `fenestra.popup.*` pair are handled
// by the host itself rather than the app's `BridgeRuntime`, so this
// module sits between `bridge::handle_navigation_starting` and the
// `GuestManager`. Everything else returns `None` and falls through to
// the app's own bridge handlers.

#![cfg(target_os = "windows")]

use std::sync::Arc;

use fenestra_bridge::{
    BridgeCommand, BridgeError, BridgeResponse, BridgeResult, POPUP_CLOSE_COMMAND,
    POPUP_OPEN_COMMAND,
    guest::{
        GuestBounds, GuestCreateOptions, GuestHostControl, GuestPopupPolicy, POPUP_GUEST_ID,
        default_partition_for, is_guest_command, ok_empty, ok_info, ok_list,
    },
};
use serde_json::{Value, json};

use crate::{WebView2Error, WebView2ProcessInner, windows::guest::GuestManager};

/// Route `fenestra.guest.*` / `fenestra.popup.*` to the guest manager.
/// Returns `None` for every other bridge command.
pub(crate) fn dispatch(
    inner: &Arc<WebView2ProcessInner>,
    command: &BridgeCommand,
) -> Option<BridgeResult> {
    if command.name == POPUP_OPEN_COMMAND {
        return Some(open_popup(inner, command));
    }
    if command.name == POPUP_CLOSE_COMMAND {
        return Some(with_manager(inner, |manager| {
            manager
                .destroy_with_inner(inner, POPUP_GUEST_ID)
                .map_err(bridge_error)?;
            ok_empty()
        }));
    }
    if command.name == "fenestra.guest.setCovered" {
        let covered = command
            .params
            .get("covered")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        return Some(with_manager(inner, |manager| {
            manager.set_covered(inner, covered);
            ok_empty()
        }));
    }
    if command.name == "fenestra.guest.capturePreview" {
        let id = command
            .params
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            return Some(Err(BridgeError::new(
                "fenestra.guest.capturePreview requires `id`",
            )));
        }
        return Some(with_manager(inner, |manager| {
            let data_url = manager.capture_preview(&id).map_err(bridge_error)?;
            Ok(BridgeResponse::json(json!({ "dataUrl": data_url })))
        }));
    }
    if !is_guest_command(&command.name) {
        return None;
    }
    let control = match GuestHostControl::from_bridge_command(command) {
        Ok(control) => control,
        Err(error) => return Some(Err(error)),
    };
    Some(with_manager(inner, |manager| {
        apply(inner, manager, control)
    }))
}

/// Create a guest for a `window.open` that a guest's popup policy routed
/// to `openGuest`. Runs on the launch loop so guest creation never
/// happens inside another guest's WebView2 callback.
pub(crate) fn open_requested_guest(inner: &Arc<WebView2ProcessInner>, parent: &str, url: &str) {
    let Ok(mut manager) = inner.guests.try_lock() else {
        return;
    };
    let Ok(parent_info) = manager.info(parent) else {
        return;
    };
    let options = GuestCreateOptions {
        url: Some(url.to_string()),
        bounds: cascade(parent_info.bounds),
        partition: manager.partition_of(parent),
        popup_policy: parent_info.popup_policy,
        ..GuestCreateOptions::default()
    };
    if let Err(error) = manager.create(inner, options) {
        eprintln!("fenestra: guest popup could not be opened: {error}");
    }
}

fn apply(
    inner: &Arc<WebView2ProcessInner>,
    manager: &mut GuestManager,
    control: GuestHostControl,
) -> BridgeResult {
    match control {
        GuestHostControl::Create(options) => {
            let info = manager.create(inner, options).map_err(bridge_error)?;
            let primary = inner
                .primary_host
                .load(std::sync::atomic::Ordering::Relaxed);
            let _ = manager.raise_above_primary(&info.id, primary);
            ok_info(info)
        }
        GuestHostControl::Destroy { id } => {
            manager
                .destroy_with_inner(inner, &id)
                .map_err(bridge_error)?;
            ok_empty()
        }
        GuestHostControl::Navigate { id, url } => {
            manager.navigate(&id, &url).map_err(bridge_error)?;
            ok_empty()
        }
        GuestHostControl::SetBounds { id, bounds } => {
            manager
                .set_bounds(inner, &id, bounds)
                .map_err(bridge_error)?;
            let primary = inner
                .primary_host
                .load(std::sync::atomic::Ordering::Relaxed);
            let _ = manager.raise_above_primary(&id, primary);
            ok_empty()
        }
        GuestHostControl::SetVisible { id, visible } => {
            manager
                .set_visible(inner, &id, visible)
                .map_err(bridge_error)?;
            if visible {
                let primary = inner
                    .primary_host
                    .load(std::sync::atomic::Ordering::Relaxed);
                let _ = manager.raise_above_primary(&id, primary);
            }
            ok_empty()
        }
        GuestHostControl::Focus { id } => {
            manager.focus(&id).map_err(bridge_error)?;
            ok_empty()
        }
        GuestHostControl::Reload { id, ignore_cache } => {
            manager.reload(&id, ignore_cache).map_err(bridge_error)?;
            ok_empty()
        }
        GuestHostControl::GoBack { id } => {
            manager.go_back(&id).map_err(bridge_error)?;
            ok_empty()
        }
        GuestHostControl::GoForward { id } => {
            manager.go_forward(&id).map_err(bridge_error)?;
            ok_empty()
        }
        GuestHostControl::SetZoom { id, factor } => {
            manager.set_zoom(&id, factor).map_err(bridge_error)?;
            ok_empty()
        }
        GuestHostControl::ExecuteJavaScript { id, code } => {
            let result = manager
                .execute_javascript(&id, &code)
                .map_err(bridge_error)?;
            Ok(BridgeResponse::json(json!({ "result": result })))
        }
        GuestHostControl::DownloadAction {
            download_id,
            action,
            save_path,
        } => {
            manager
                .download_action(&download_id, action, save_path.as_deref())
                .map_err(bridge_error)?;
            Ok(BridgeResponse::json(json!({
                "downloadId": download_id,
                "action": action.as_str(),
            })))
        }
        GuestHostControl::List => ok_list(&manager.list()),
        GuestHostControl::Get { id } => ok_info(manager.info(&id).map_err(bridge_error)?),
    }
}

fn open_popup(inner: &Arc<WebView2ProcessInner>, command: &BridgeCommand) -> BridgeResult {
    let html = string_param(&command.params, "html");
    let url = string_param(&command.params, "url");
    if html.is_none() && url.is_none() {
        return Err(BridgeError::new(
            "fenestra.popup.open requires `html` or `url`",
        ));
    }
    let options = GuestCreateOptions {
        id: Some(POPUP_GUEST_ID.to_string()),
        // Inline popup markup comes from the app itself, so it keeps the
        // bridge. A popup pointed at a URL is treated like any other
        // guest and gets no privileged surface.
        allow_bridge: html.is_some(),
        bounds: popup_bounds(&command.params),
        html,
        url,
        partition: Some(default_partition_for(POPUP_GUEST_ID)),
        popup_policy: GuestPopupPolicy::Deny,
        ..GuestCreateOptions::default()
    };
    with_manager(inner, move |manager| {
        ok_info(manager.create(inner, options).map_err(bridge_error)?)
    })
}

fn with_manager<F>(inner: &Arc<WebView2ProcessInner>, action: F) -> BridgeResult
where
    F: FnOnce(&mut GuestManager) -> BridgeResult,
{
    // Guest creation pumps Win32 messages, which can deliver another
    // bridge command on this thread. `try_lock` turns that re-entry into
    // a retryable error instead of a deadlock.
    let Ok(mut manager) = inner.guests.try_lock() else {
        return Err(BridgeError::new(
            "guest host is busy handling another guest command",
        ));
    };
    action(&mut manager)
}

/// Offset a child popup so it does not land exactly on its opener.
fn cascade(bounds: GuestBounds) -> GuestBounds {
    GuestBounds::new(
        bounds.x.saturating_add(24),
        bounds.y.saturating_add(24),
        bounds.width,
        bounds.height,
    )
}

fn popup_bounds(params: &Value) -> GuestBounds {
    GuestBounds::new(
        params.get("x").and_then(Value::as_i64).unwrap_or(0) as i32,
        params.get("y").and_then(Value::as_i64).unwrap_or(0) as i32,
        params.get("width").and_then(Value::as_u64).unwrap_or(1) as u32,
        params.get("height").and_then(Value::as_u64).unwrap_or(1) as u32,
    )
}

fn string_param(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn bridge_error(error: WebView2Error) -> BridgeError {
    BridgeError::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_bounds_default_to_unit_size() {
        assert_eq!(popup_bounds(&json!({})), GuestBounds::new(0, 0, 1, 1));
    }

    #[test]
    fn popup_bounds_read_flat_fields() {
        let bounds = popup_bounds(&json!({ "x": 12, "y": 34, "width": 400, "height": 300 }));
        assert_eq!(bounds, GuestBounds::new(12, 34, 400, 300));
    }

    #[test]
    fn cascade_offsets_child_popups() {
        assert_eq!(
            cascade(GuestBounds::new(10, 20, 800, 600)),
            GuestBounds::new(34, 44, 800, 600)
        );
    }
}
