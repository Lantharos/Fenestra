//! Guest host-control wire payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bridge::{BridgeCommand, BridgeError};
use crate::guest::{
    CREATE_COMMAND, DESTROY_COMMAND, DOWNLOAD_ACTION_COMMAND, EXECUTE_JS_COMMAND, FOCUS_COMMAND,
    GET_COMMAND, GO_BACK_COMMAND, GO_FORWARD_COMMAND, LIST_COMMAND, NAVIGATE_COMMAND,
    RELOAD_COMMAND, SET_BOUNDS_COMMAND, SET_VISIBLE_COMMAND, SET_ZOOM_COMMAND,
};
use crate::guest_create::{
    GuestBounds, GuestCreateOptions, bool_field, int_field, normalize_guest_id, uint_field,
};
use crate::guest_download::GuestDownloadAction;

/// Host-control payloads sent over `SABINE_HOST_CONTROL` for guests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum GuestHostControl {
    Create(GuestCreateOptions),
    Destroy {
        id: String,
    },
    Navigate {
        id: String,
        url: String,
    },
    SetBounds {
        id: String,
        bounds: GuestBounds,
    },
    SetVisible {
        id: String,
        visible: bool,
    },
    Focus {
        id: String,
    },
    Reload {
        id: String,
        ignore_cache: bool,
    },
    GoBack {
        id: String,
    },
    GoForward {
        id: String,
    },
    SetZoom {
        id: String,
        factor: f64,
    },
    ExecuteJavaScript {
        id: String,
        code: String,
    },
    DownloadAction {
        download_id: String,
        action: GuestDownloadAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        save_path: Option<String>,
    },
    List,
    Get {
        id: String,
    },
}

impl GuestHostControl {
    pub fn command_name(&self) -> &'static str {
        match self {
            Self::Create(_) => "guest.create",
            Self::Destroy { .. } => "guest.destroy",
            Self::Navigate { .. } => "guest.navigate",
            Self::SetBounds { .. } => "guest.setBounds",
            Self::SetVisible { .. } => "guest.setVisible",
            Self::Focus { .. } => "guest.focus",
            Self::Reload { .. } => "guest.reload",
            Self::GoBack { .. } => "guest.goBack",
            Self::GoForward { .. } => "guest.goForward",
            Self::SetZoom { .. } => "guest.setZoom",
            Self::ExecuteJavaScript { .. } => "guest.executeJavaScript",
            Self::DownloadAction { .. } => "guest.downloadAction",
            Self::List => "guest.list",
            Self::Get { .. } => "guest.get",
        }
    }

    pub fn to_host_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_bridge_command(command: &BridgeCommand) -> Result<Self, BridgeError> {
        match command.name.as_str() {
            CREATE_COMMAND => Ok(Self::Create(GuestCreateOptions::from_value(
                &command.params,
            )?)),
            DESTROY_COMMAND => Ok(Self::Destroy {
                id: required_id(&command.params)?,
            }),
            NAVIGATE_COMMAND => Ok(Self::Navigate {
                id: required_id(&command.params)?,
                url: required_string(&command.params, "url")?,
            }),
            SET_BOUNDS_COMMAND => Ok(Self::SetBounds {
                id: required_id(&command.params)?,
                bounds: bounds_from_value(&command.params)?,
            }),
            SET_VISIBLE_COMMAND => Ok(Self::SetVisible {
                id: required_id(&command.params)?,
                visible: bool_field(&command.params, "visible").unwrap_or(true),
            }),
            FOCUS_COMMAND => Ok(Self::Focus {
                id: required_id(&command.params)?,
            }),
            RELOAD_COMMAND => Ok(Self::Reload {
                id: required_id(&command.params)?,
                ignore_cache: bool_field(&command.params, "ignoreCache")
                    .or_else(|| bool_field(&command.params, "ignore_cache"))
                    .unwrap_or(false),
            }),
            GO_BACK_COMMAND => Ok(Self::GoBack {
                id: required_id(&command.params)?,
            }),
            GO_FORWARD_COMMAND => Ok(Self::GoForward {
                id: required_id(&command.params)?,
            }),
            SET_ZOOM_COMMAND => Ok(Self::SetZoom {
                id: required_id(&command.params)?,
                factor: command
                    .params
                    .get("factor")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0)
                    .clamp(0.25, 5.0),
            }),
            EXECUTE_JS_COMMAND => Ok(Self::ExecuteJavaScript {
                id: required_id(&command.params)?,
                code: required_string(&command.params, "code")?,
            }),
            DOWNLOAD_ACTION_COMMAND => {
                let action_name = required_string(&command.params, "action")?;
                let action = GuestDownloadAction::parse(&action_name).ok_or_else(|| {
                    BridgeError::new(format!("unknown download action: {action_name}"))
                })?;
                Ok(Self::DownloadAction {
                    download_id: required_string(&command.params, "downloadId")
                        .or_else(|_| required_string(&command.params, "download_id"))?,
                    action,
                    save_path: command
                        .params
                        .get("savePath")
                        .or_else(|| command.params.get("save_path"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            }
            LIST_COMMAND => Ok(Self::List),
            GET_COMMAND => Ok(Self::Get {
                id: required_id(&command.params)?,
            }),
            _ => Err(BridgeError::new(format!(
                "not a guest bridge command: {}",
                command.name
            ))),
        }
    }
}

fn required_id(params: &Value) -> Result<String, BridgeError> {
    normalize_guest_id(
        params
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("guest id is required"))?,
    )
}

fn required_string(params: &Value, key: &str) -> Result<String, BridgeError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| BridgeError::new(format!("missing or empty `{key}`")))
}

fn bounds_from_value(value: &Value) -> Result<GuestBounds, BridgeError> {
    if let Some(bounds) = value.get("bounds") {
        return Ok(GuestBounds::new(
            int_field(bounds, "x").unwrap_or(0),
            int_field(bounds, "y").unwrap_or(0),
            uint_field(bounds, "width").unwrap_or(1),
            uint_field(bounds, "height").unwrap_or(1),
        )
        .normalized());
    }
    Ok(GuestBounds::new(
        int_field(value, "x").unwrap_or(0),
        int_field(value, "y").unwrap_or(0),
        uint_field(value, "width").unwrap_or(1),
        uint_field(value, "height").unwrap_or(1),
    )
    .normalized())
}
