//! Guest download event and action types.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Download lifecycle event emitted to the primary surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestDownloadEvent {
    pub guest_id: String,
    pub download_id: String,
    pub url: String,
    pub filename: String,
    pub mime_type: String,
    pub total_bytes: i64,
    pub received_bytes: i64,
    pub state: GuestDownloadState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GuestDownloadState {
    Requested,
    Progress,
    Completed,
    Cancelled,
    Interrupted,
}

impl GuestDownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Progress => "progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

impl GuestDownloadEvent {
    pub fn to_json(&self) -> Value {
        json!({
            "guestId": self.guest_id,
            "downloadId": self.download_id,
            "url": self.url,
            "filename": self.filename,
            "mimeType": self.mime_type,
            "totalBytes": self.total_bytes,
            "receivedBytes": self.received_bytes,
            "state": self.state.as_str(),
            "savePath": self.save_path,
            "error": self.error,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GuestDownloadAction {
    Accept,
    Cancel,
    Pause,
    Resume,
}

impl GuestDownloadAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "accept" | "allow" => Some(Self::Accept),
            "cancel" | "deny" | "reject" => Some(Self::Cancel),
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Cancel => "cancel",
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }
}
