// WebView2 event wiring for guest webviews.
//
// Each guest gets its own set of handlers. They keep the shared
// `GuestState` snapshot current and forward lifecycle changes to the
// primary page as `guest.*` bridge events. Events are queued on the
// launch loop's `WebView2UserEvent` channel rather than executed inline,
// so a guest callback never re-enters `ExecuteScript` on the primary
// webview while WebView2 is still dispatching.

#![cfg(target_os = "windows")]

use std::sync::{Arc, Mutex, mpsc::Sender};

use fenestra_bridge::guest::{GuestDownloadState, GuestPopupPolicy};
use serde_json::{Value, json};
use webview2_com::{
    BytesReceivedChangedEventHandler, DocumentTitleChangedEventHandler, DownloadStartingEventHandler,
    HistoryChangedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON, COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED,
        COREWEBVIEW2_DOWNLOAD_STATE, COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
        COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED, ICoreWebView2, ICoreWebView2DownloadOperation,
        ICoreWebView2DownloadStartingEventArgs, ICoreWebView2NewWindowRequestedEventArgs,
        ICoreWebView2_4,
    },
    NavigationCompletedEventHandler, NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    SourceChangedEventHandler, StateChangedEventHandler,
};
use windows::core::Interface;

use crate::{
    WebView2Result,
    windows::{
        bridge::{read_pwstr, webview2_error, wide_pwstr},
        guest::{DownloadRegistry, GuestState},
        launch::WebView2UserEvent,
    },
};

/// Everything a guest's WebView2 handlers need. Cloned into each
/// closure; holds no reference back to the guest map, so handlers can
/// run while the guest manager is locked.
#[derive(Clone)]
pub(crate) struct GuestEventContext {
    pub(crate) id: String,
    pub(crate) state: Arc<Mutex<GuestState>>,
    pub(crate) events: Sender<WebView2UserEvent>,
    pub(crate) downloads: Arc<Mutex<DownloadRegistry>>,
    pub(crate) popup_policy: GuestPopupPolicy,
    pub(crate) allow_downloads: bool,
}

impl GuestEventContext {
    fn emit(&self, name: &str, payload: Value) {
        emit_guest_event(&self.events, name, payload);
    }
}

pub(crate) fn emit_guest_event(events: &Sender<WebView2UserEvent>, name: &str, payload: Value) {
    WebView2UserEvent::BridgeEvent {
        name: name.to_string(),
        payload,
    }
    .dispatch(events);
}

pub(crate) fn register_guest_handlers(
    webview: &ICoreWebView2,
    context: GuestEventContext,
) -> WebView2Result<()> {
    register_navigation_starting(webview, context.clone())?;
    register_navigation_completed(webview, context.clone())?;
    register_source_changed(webview, context.clone())?;
    register_history_changed(webview, context.clone())?;
    register_title_changed(webview, context.clone())?;
    register_new_window_requested(webview, context.clone())?;
    register_download_starting(webview, context)
}

fn register_navigation_starting(
    webview: &ICoreWebView2,
    context: GuestEventContext,
) -> WebView2Result<()> {
    let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
        let url = args
            .and_then(|args| read_pwstr(|out| unsafe { args.Uri(out) }))
            .unwrap_or_default();
        if let Ok(mut state) = context.state.lock() {
            state.loading = true;
            if !url.is_empty() {
                state.url = url.clone();
            }
        }
        context.emit(
            "guest.loading",
            json!({ "id": context.id, "loading": true, "url": url }),
        );
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_NavigationStarting(&handler, &mut token) }.map_err(webview2_error)
}

fn register_navigation_completed(
    webview: &ICoreWebView2,
    context: GuestEventContext,
) -> WebView2Result<()> {
    let handler = NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
        let success = args
            .and_then(|args| read_bool(|out| unsafe { args.IsSuccess(out) }))
            .unwrap_or(true);
        if let Some(sender) = sender.as_ref() {
            store_navigation_state(&context, sender, Some(false));
        }
        context.emit("guest.navigated", navigation_payload(&context, Some(success)));
        context.emit(
            "guest.loading",
            json!({ "id": context.id, "loading": false, "success": success }),
        );
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_NavigationCompleted(&handler, &mut token) }.map_err(webview2_error)
}

fn register_source_changed(
    webview: &ICoreWebView2,
    context: GuestEventContext,
) -> WebView2Result<()> {
    let handler = SourceChangedEventHandler::create(Box::new(move |sender, _args| {
        if let Some(sender) = sender.as_ref() {
            store_navigation_state(&context, sender, None);
        }
        context.emit("guest.navigated", navigation_payload(&context, None));
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_SourceChanged(&handler, &mut token) }.map_err(webview2_error)
}

fn register_history_changed(
    webview: &ICoreWebView2,
    context: GuestEventContext,
) -> WebView2Result<()> {
    let handler = HistoryChangedEventHandler::create(Box::new(move |sender, _args| {
        if let Some(sender) = sender.as_ref() {
            store_navigation_state(&context, sender, None);
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_HistoryChanged(&handler, &mut token) }.map_err(webview2_error)
}

fn register_title_changed(
    webview: &ICoreWebView2,
    context: GuestEventContext,
) -> WebView2Result<()> {
    let handler = DocumentTitleChangedEventHandler::create(Box::new(move |sender, _args| {
        let title = sender
            .as_ref()
            .and_then(|sender| read_pwstr(|out| unsafe { sender.DocumentTitle(out) }))
            .unwrap_or_default();
        if let Ok(mut state) = context.state.lock() {
            state.title = title.clone();
        }
        context.emit("guest.title", json!({ "id": context.id, "title": title }));
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_DocumentTitleChanged(&handler, &mut token) }.map_err(webview2_error)
}

fn register_new_window_requested(
    webview: &ICoreWebView2,
    context: GuestEventContext,
) -> WebView2Result<()> {
    let handler = NewWindowRequestedEventHandler::create(Box::new(move |sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let url = read_pwstr(|out| unsafe { args.Uri(out) }).unwrap_or_default();
        let user_initiated =
            read_bool(|out| unsafe { args.IsUserInitiated(out) }).unwrap_or(false);
        apply_popup_policy(&context, sender.as_ref(), &args, &url);
        context.emit(
            "guest.newWindow",
            json!({
                "id": context.id,
                "url": url,
                "policy": context.popup_policy.as_str(),
                "userInitiated": user_initiated,
            }),
        );
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_NewWindowRequested(&handler, &mut token) }.map_err(webview2_error)
}

fn apply_popup_policy(
    context: &GuestEventContext,
    sender: Option<&ICoreWebView2>,
    args: &ICoreWebView2NewWindowRequestedEventArgs,
    url: &str,
) {
    match context.popup_policy {
        // WebView2 opens its own popup window; the app only observes it.
        GuestPopupPolicy::Allow => {}
        GuestPopupPolicy::Deny => {
            let _ = unsafe { args.SetHandled(true) };
        }
        GuestPopupPolicy::NavigateSame => {
            let _ = unsafe { args.SetHandled(true) };
            if let Some(sender) = sender.filter(|_| !url.is_empty()) {
                let wide = wide_pwstr(url);
                let _ = unsafe { sender.Navigate(windows::core::PCWSTR(wide.as_ptr())) };
            }
        }
        GuestPopupPolicy::OpenGuest => {
            let _ = unsafe { args.SetHandled(true) };
            if !url.is_empty() {
                WebView2UserEvent::GuestOpenRequested {
                    parent: context.id.clone(),
                    url: url.to_string(),
                }
                .dispatch(&context.events);
            }
        }
    }
}

fn register_download_starting(
    webview: &ICoreWebView2,
    context: GuestEventContext,
) -> WebView2Result<()> {
    let Ok(webview4) = webview.cast::<ICoreWebView2_4>() else {
        return Ok(());
    };
    let handler = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
        if let Some(args) = args {
            handle_download_starting(&context, &args);
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview4.add_DownloadStarting(&handler, &mut token) }.map_err(webview2_error)
}

/// Hold the download until the app answers with
/// `fenestra.guest.downloadAction`.
///
/// WebView2 fixes the target path while `DownloadStarting` is still
/// pending, so a deferral is the only way an app can choose where a file
/// lands. `SetHandled(true)` also suppresses the built-in download
/// bubble, leaving the download UI entirely to the app.
fn handle_download_starting(
    context: &GuestEventContext,
    args: &ICoreWebView2DownloadStartingEventArgs,
) {
    let operation = unsafe { args.DownloadOperation() };
    let Ok(operation) = operation else {
        return;
    };
    let url = read_pwstr(|out| unsafe { operation.Uri(out) }).unwrap_or_default();
    let save_path = read_pwstr(|out| unsafe { args.ResultFilePath(out) }).unwrap_or_default();
    let mime_type = read_pwstr(|out| unsafe { operation.MimeType(out) }).unwrap_or_default();
    let total_bytes = read_i64(|out| unsafe { operation.TotalBytesToReceive(out) });

    if !context.allow_downloads {
        let _ = unsafe { args.SetCancel(true) };
        let _ = unsafe { args.SetHandled(true) };
        context.emit(
            "guest.download",
            json!({
                "guestId": context.id,
                "downloadId": Value::Null,
                "url": url,
                "filename": file_name_of(&save_path),
                "mimeType": mime_type,
                "totalBytes": total_bytes,
                "receivedBytes": 0,
                "state": GuestDownloadState::Cancelled.as_str(),
                "error": "downloads are disabled for this guest",
            }),
        );
        return;
    }

    let deferral = unsafe { args.GetDeferral() };
    let Ok(deferral) = deferral else {
        let _ = unsafe { args.SetCancel(true) };
        return;
    };
    let _ = unsafe { args.SetHandled(true) };
    let Ok(mut registry) = context.downloads.lock() else {
        let _ = unsafe { args.SetCancel(true) };
        let _ = unsafe { deferral.Complete() };
        return;
    };
    let download_id = registry.register(&context.id, operation.clone(), args.clone(), deferral);
    drop(registry);

    register_download_progress(context, &operation, download_id.clone());
    context.emit(
        "guest.download",
        json!({
            "guestId": context.id,
            "downloadId": download_id,
            "url": url,
            "filename": file_name_of(&save_path),
            "mimeType": mime_type,
            "totalBytes": total_bytes,
            "receivedBytes": 0,
            "state": GuestDownloadState::Requested.as_str(),
            "savePath": save_path,
        }),
    );
}

fn register_download_progress(
    context: &GuestEventContext,
    operation: &ICoreWebView2DownloadOperation,
    download_id: String,
) {
    let bytes_context = context.clone();
    let bytes_id = download_id.clone();
    let bytes_handler = BytesReceivedChangedEventHandler::create(Box::new(move |sender, _args| {
        if let Some(sender) = sender.as_ref() {
            bytes_context.emit(
                "guest.download",
                download_payload(&bytes_context.id, &bytes_id, sender, None),
            );
        }
        Ok(())
    }));
    let mut bytes_token = 0i64;
    let _ = unsafe { operation.add_BytesReceivedChanged(&bytes_handler, &mut bytes_token) };

    let state_context = context.clone();
    let state_handler = StateChangedEventHandler::create(Box::new(move |sender, _args| {
        let Some(sender) = sender.as_ref() else {
            return Ok(());
        };
        let state = download_state(sender);
        state_context.emit(
            "guest.download",
            download_payload(&state_context.id, &download_id, sender, state),
        );
        if state.is_some_and(|state| state != GuestDownloadState::Progress)
            && let Ok(mut registry) = state_context.downloads.lock()
        {
            registry.forget(&download_id);
        }
        Ok(())
    }));
    let mut state_token = 0i64;
    let _ = unsafe { operation.add_StateChanged(&state_handler, &mut state_token) };
}

fn download_payload(
    guest_id: &str,
    download_id: &str,
    operation: &ICoreWebView2DownloadOperation,
    state: Option<GuestDownloadState>,
) -> Value {
    let save_path = read_pwstr(|out| unsafe { operation.ResultFilePath(out) }).unwrap_or_default();
    json!({
        "guestId": guest_id,
        "downloadId": download_id,
        "url": read_pwstr(|out| unsafe { operation.Uri(out) }).unwrap_or_default(),
        "filename": file_name_of(&save_path),
        "mimeType": read_pwstr(|out| unsafe { operation.MimeType(out) }).unwrap_or_default(),
        "totalBytes": read_i64(|out| unsafe { operation.TotalBytesToReceive(out) }),
        "receivedBytes": read_i64(|out| unsafe { operation.BytesReceived(out) }),
        "state": state.unwrap_or(GuestDownloadState::Progress).as_str(),
        "savePath": save_path,
    })
}

fn download_state(operation: &ICoreWebView2DownloadOperation) -> Option<GuestDownloadState> {
    let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
    unsafe { operation.State(&mut state) }.ok()?;
    if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
        return Some(GuestDownloadState::Completed);
    }
    if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
        let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
        if unsafe { operation.InterruptReason(&mut reason) }.is_ok()
            && reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED
        {
            return Some(GuestDownloadState::Cancelled);
        }
        return Some(GuestDownloadState::Interrupted);
    }
    Some(GuestDownloadState::Progress)
}

fn store_navigation_state(
    context: &GuestEventContext,
    webview: &ICoreWebView2,
    loading: Option<bool>,
) {
    let url = read_pwstr(|out| unsafe { webview.Source(out) }).unwrap_or_default();
    let can_go_back = read_bool(|out| unsafe { webview.CanGoBack(out) }).unwrap_or(false);
    let can_go_forward = read_bool(|out| unsafe { webview.CanGoForward(out) }).unwrap_or(false);
    let Ok(mut state) = context.state.lock() else {
        return;
    };
    if !url.is_empty() {
        state.url = url;
    }
    state.can_go_back = can_go_back;
    state.can_go_forward = can_go_forward;
    if let Some(loading) = loading {
        state.loading = loading;
    }
}

fn navigation_payload(context: &GuestEventContext, success: Option<bool>) -> Value {
    let Ok(state) = context.state.lock() else {
        return json!({ "id": context.id });
    };
    json!({
        "id": context.id,
        "url": state.url,
        "title": state.title,
        "canGoBack": state.can_go_back,
        "canGoForward": state.can_go_forward,
        "success": success,
    })
}

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn read_i64<F: FnOnce(*mut i64) -> windows::core::Result<()>>(read: F) -> i64 {
    let mut value = 0i64;
    match read(&mut value) {
        Ok(()) => value,
        Err(_) => 0,
    }
}

fn read_bool<F: FnOnce(*mut windows::core::BOOL) -> windows::core::Result<()>>(
    read: F,
) -> Option<bool> {
    let mut value = windows::core::BOOL(0);
    read(&mut value).ok()?;
    Some(value.as_bool())
}
