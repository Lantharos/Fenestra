//! Guest webview bridge command names and helpers.
//!
//! Guests are secondary Chromium views hosted inside a single
//! window. The app UI creates and controls them via `mullion.guest.*`
//! bridge commands.

use serde_json::json;

use crate::bridge::{BridgeResponse, BridgeResult};

pub use crate::guest_create::{
    GuestBounds, GuestCreateOptions, GuestInfo, GuestPopupPolicy, default_partition_for,
    normalize_guest_id,
};
pub use crate::guest_download::{GuestDownloadAction, GuestDownloadEvent, GuestDownloadState};
pub use crate::guest_host_control::GuestHostControl;

pub const CREATE_COMMAND: &str = "mullion.guest.create";
pub const DESTROY_COMMAND: &str = "mullion.guest.destroy";
pub const NAVIGATE_COMMAND: &str = "mullion.guest.navigate";
pub const SET_BOUNDS_COMMAND: &str = "mullion.guest.setBounds";
pub const SET_VISIBLE_COMMAND: &str = "mullion.guest.setVisible";
pub const SET_COVERED_COMMAND: &str = "mullion.guest.setCovered";
pub const CAPTURE_PREVIEW_COMMAND: &str = "mullion.guest.capturePreview";
pub const FOCUS_COMMAND: &str = "mullion.guest.focus";
pub const RELOAD_COMMAND: &str = "mullion.guest.reload";
pub const GO_BACK_COMMAND: &str = "mullion.guest.goBack";
pub const GO_FORWARD_COMMAND: &str = "mullion.guest.goForward";
pub const LIST_COMMAND: &str = "mullion.guest.list";
pub const GET_COMMAND: &str = "mullion.guest.get";
pub const SET_ZOOM_COMMAND: &str = "mullion.guest.setZoom";
pub const EXECUTE_JS_COMMAND: &str = "mullion.guest.executeJavaScript";
pub const DOWNLOAD_ACTION_COMMAND: &str = "mullion.guest.downloadAction";

/// Reserved guest id used by the legacy `mullion.popup` surface.
pub const POPUP_GUEST_ID: &str = "__mullion_popup";

const INTERNAL_COMMANDS: [&str; 16] = [
    CREATE_COMMAND,
    DESTROY_COMMAND,
    NAVIGATE_COMMAND,
    SET_BOUNDS_COMMAND,
    SET_VISIBLE_COMMAND,
    SET_COVERED_COMMAND,
    CAPTURE_PREVIEW_COMMAND,
    FOCUS_COMMAND,
    RELOAD_COMMAND,
    GO_BACK_COMMAND,
    GO_FORWARD_COMMAND,
    LIST_COMMAND,
    GET_COMMAND,
    SET_ZOOM_COMMAND,
    EXECUTE_JS_COMMAND,
    DOWNLOAD_ACTION_COMMAND,
];

/// Append guest bridge commands to an allow-list.
pub fn bridge_commands_with_guest(mut commands: Vec<String>) -> Vec<String> {
    for command in INTERNAL_COMMANDS {
        if !commands.iter().any(|existing| existing == command) {
            commands.push(command.to_string());
        }
    }
    commands
}

pub fn is_guest_command(name: &str) -> bool {
    INTERNAL_COMMANDS.contains(&name)
}

pub fn ok_info(info: GuestInfo) -> BridgeResult {
    Ok(BridgeResponse::json(info.to_json()))
}

pub fn ok_list(guests: &[GuestInfo]) -> BridgeResult {
    Ok(BridgeResponse::json(json!({
        "guests": guests.iter().map(GuestInfo::to_json).collect::<Vec<_>>(),
    })))
}

pub fn ok_empty() -> BridgeResult {
    Ok(BridgeResponse::json(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeCommand;
    use serde_json::json;

    #[test]
    fn create_options_require_url_or_html() {
        let error = GuestCreateOptions::from_value(&json!({
            "x": 10, "y": 20, "width": 800, "height": 600
        }))
        .unwrap_err();
        assert!(error.to_string().contains("url"));
    }

    #[test]
    fn create_options_parse_flat_bounds() {
        let options = GuestCreateOptions::from_value(&json!({
            "id": "browser-1",
            "url": "https://example.com",
            "x": 10,
            "y": 20,
            "width": 800,
            "height": 600,
            "popupPolicy": "deny",
        }))
        .unwrap();
        assert_eq!(options.id.as_deref(), Some("browser-1"));
        assert_eq!(options.bounds, GuestBounds::new(10, 20, 800, 600));
        assert_eq!(options.popup_policy, GuestPopupPolicy::Deny);
    }

    #[test]
    fn create_options_parse_input_policy() {
        let options = GuestCreateOptions::from_value(&json!({
            "url": "https://example.com",
            "bounds": { "x": 0, "y": 0, "width": 800, "height": 600 },
            "interceptedShortcuts": ["Primary+T", "Primary+K"],
            "interceptHorizontalWheel": true,
        }))
        .unwrap();
        assert_eq!(
            options.intercepted_shortcuts,
            vec!["Primary+T".to_string(), "Primary+K".to_string()]
        );
        assert!(options.intercept_horizontal_wheel);
    }

    #[test]
    fn guest_commands_are_appended() {
        let commands = bridge_commands_with_guest(vec!["notes.list".into()]);
        assert!(commands.iter().any(|c| c == CREATE_COMMAND));
        assert!(commands.iter().any(|c| c == "notes.list"));
    }

    #[test]
    fn host_control_round_trips_create() {
        let command = BridgeCommand {
            name: CREATE_COMMAND.into(),
            params: json!({
                "url": "https://example.com",
                "x": 0,
                "y": 40,
                "width": 1200,
                "height": 800,
            }),
            origin: None,
        };
        let control = GuestHostControl::from_bridge_command(&command).unwrap();
        assert_eq!(control.command_name(), "guest.create");
    }
}
