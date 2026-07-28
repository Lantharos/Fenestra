// Guest webviews for the WebView2 backend.
//
// A guest is a secondary Chromium view hosted inside the same window as
// the app UI. The primary page drives them through the
// `fenestra.guest.*` bridge commands, which `guest_commands` routes into
// the `GuestManager` below.
//
// Guests are untrusted by default: the Fenestra bridge script is only
// installed when the caller passes `allowBridge: true`, and every
// partition gets its own WebView2 environment — and therefore its own
// cookie jar and cache — under `<profile root>/guests/<partition hash>`.
//
// Every method here runs on the UI thread. Bridge commands arrive from
// WebView2's `NavigationStarting` callback, which WebView2 raises there.

#![cfg(target_os = "windows")]

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering, mpsc::Sender},
};

use fenestra_bridge::guest::{
    GuestBounds, GuestCreateOptions, GuestDownloadAction, GuestInfo, GuestPopupPolicy,
    default_partition_for,
};
use serde_json::{Value, json};
use webview2_com::{
    ExecuteScriptCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_COLOR, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC, ICoreWebView2,
        ICoreWebView2Controller, ICoreWebView2Controller2, ICoreWebView2Deferral,
        ICoreWebView2DownloadOperation, ICoreWebView2DownloadStartingEventArgs,
        ICoreWebView2Environment,
    },
};
use windows::core::Interface;

use crate::{
    WebView2Error, WebView2ProcessInner, WebView2Result,
    windows::{
        bridge, composition,
        guest_events::{self, GuestEventContext},
        guest_host,
        launch::{WebView2UserEvent, stable_hash},
    },
};

/// Mutable snapshot of one guest, shared with its WebView2 handlers.
#[derive(Clone, Debug)]
pub(crate) struct GuestState {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) bounds: GuestBounds,
    pub(crate) visible: bool,
    pub(crate) loading: bool,
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
    pub(crate) zoom_factor: f64,
}

/// A download that WebView2 is holding open until the app answers with
/// `fenestra.guest.downloadAction`.
struct PendingDownload {
    args: ICoreWebView2DownloadStartingEventArgs,
    deferral: ICoreWebView2Deferral,
}

impl PendingDownload {
    fn accept(self, save_path: Option<&str>) -> WebView2Result<()> {
        if let Some(save_path) = save_path {
            let wide = bridge::wide_pwstr(save_path);
            unsafe {
                self.args
                    .SetResultFilePath(windows::core::PCWSTR(wide.as_ptr()))
            }
            .map_err(bridge::webview2_error)?;
        }
        unsafe { self.deferral.Complete() }.map_err(bridge::webview2_error)
    }

    fn cancel(self) -> WebView2Result<()> {
        unsafe { self.args.SetCancel(true) }.map_err(bridge::webview2_error)?;
        unsafe { self.deferral.Complete() }.map_err(bridge::webview2_error)
    }
}

struct DownloadEntry {
    guest_id: String,
    operation: ICoreWebView2DownloadOperation,
    pending: Option<PendingDownload>,
}

/// Live downloads keyed by the id handed to the page.
#[derive(Default)]
pub(crate) struct DownloadRegistry {
    next_id: u64,
    entries: HashMap<String, DownloadEntry>,
}

impl DownloadRegistry {
    pub(crate) fn register(
        &mut self,
        guest_id: &str,
        operation: ICoreWebView2DownloadOperation,
        args: ICoreWebView2DownloadStartingEventArgs,
        deferral: ICoreWebView2Deferral,
    ) -> String {
        self.next_id += 1;
        let id = format!("download-{}", self.next_id);
        self.entries.insert(
            id.clone(),
            DownloadEntry {
                guest_id: guest_id.to_string(),
                operation,
                pending: Some(PendingDownload { args, deferral }),
            },
        );
        id
    }

    /// Drop a finished download. A still-pending deferral is cancelled
    /// so WebView2 is never left waiting on an answer that cannot come.
    pub(crate) fn forget(&mut self, download_id: &str) {
        let Some(entry) = self.entries.remove(download_id) else {
            return;
        };
        if let Some(pending) = entry.pending {
            let _ = pending.cancel();
        }
    }

    fn forget_guest(&mut self, guest_id: &str) {
        let ids: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.guest_id == guest_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.forget(&id);
        }
    }

    fn clear(&mut self) {
        let ids: Vec<String> = self.entries.keys().cloned().collect();
        for id in ids {
            self.forget(&id);
        }
    }

    fn apply(
        &mut self,
        download_id: &str,
        action: GuestDownloadAction,
        save_path: Option<&str>,
    ) -> WebView2Result<()> {
        if !self.entries.contains_key(download_id) {
            return Err(unknown_download(download_id));
        }
        match action {
            GuestDownloadAction::Accept => match self.take_pending(download_id) {
                Some(pending) => pending.accept(save_path),
                None if save_path.is_some() => Err(WebView2Error::Backend(format!(
                    "download {download_id} already started; `savePath` can only be set \
                     while its state is `requested`"
                ))),
                None => Ok(()),
            },
            GuestDownloadAction::Cancel => match self.take_pending(download_id) {
                Some(pending) => {
                    let result = pending.cancel();
                    self.entries.remove(download_id);
                    result
                }
                None => unsafe { self.started_operation(download_id, "cancelled")?.Cancel() }
                    .map_err(bridge::webview2_error),
            },
            GuestDownloadAction::Pause => {
                unsafe { self.started_operation(download_id, "paused")?.Pause() }
                    .map_err(bridge::webview2_error)
            }
            GuestDownloadAction::Resume => {
                unsafe { self.started_operation(download_id, "resumed")?.Resume() }
                    .map_err(bridge::webview2_error)
            }
        }
    }

    fn take_pending(&mut self, download_id: &str) -> Option<PendingDownload> {
        self.entries.get_mut(download_id)?.pending.take()
    }

    fn started_operation(
        &self,
        download_id: &str,
        verb: &str,
    ) -> WebView2Result<ICoreWebView2DownloadOperation> {
        let entry = self
            .entries
            .get(download_id)
            .ok_or_else(|| unknown_download(download_id))?;
        if entry.pending.is_some() {
            return Err(WebView2Error::Backend(format!(
                "download {download_id} has not been accepted yet and cannot be {verb}"
            )));
        }
        Ok(entry.operation.clone())
    }
}

fn unknown_download(download_id: &str) -> WebView2Error {
    WebView2Error::Backend(format!("unknown guest download: {download_id}"))
}

struct Guest {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    surface: GuestSurface,
    state: Arc<Mutex<GuestState>>,
    partition: String,
    allow_bridge: bool,
    popup_policy: GuestPopupPolicy,
}

enum GuestSurface {
    Windowed {
        hwnd: isize,
    },
    Composition {
        visual: windows::Win32::Graphics::DirectComposition::IDCompositionVisual,
        composition: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2CompositionController,
    },
}

impl Guest {
    fn info(&self, id: &str) -> GuestInfo {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        GuestInfo {
            id: id.to_string(),
            url: state.url,
            title: state.title,
            bounds: state.bounds,
            visible: state.visible,
            loading: state.loading,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            partition: self.partition.clone(),
            allow_bridge: self.allow_bridge,
            popup_policy: self.popup_policy,
            zoom_factor: state.zoom_factor,
        }
    }

    fn dispose_surface(&self, inner: &Arc<WebView2ProcessInner>) {
        match &self.surface {
            GuestSurface::Windowed { hwnd } => guest_host::destroy_host_window(*hwnd),
            GuestSurface::Composition { visual, .. } => {
                if let Ok(guard) = inner.dcomp.lock()
                    && let Some(dcomp) = guard.as_ref()
                {
                    let _ = composition::remove_guest_visual(dcomp, visual);
                }
            }
        }
    }
}

/// Owns every guest webview in a window plus the per-partition
/// environments they were created from.
pub(crate) struct GuestManager {
    guest_root: PathBuf,
    events: Sender<WebView2UserEvent>,
    command_allowlist: Vec<String>,
    environments: HashMap<String, ICoreWebView2Environment>,
    guests: HashMap<String, Guest>,
    downloads: Arc<Mutex<DownloadRegistry>>,
    next_guest: u64,
}

impl GuestManager {
    pub(crate) fn new(
        guest_root: PathBuf,
        events: Sender<WebView2UserEvent>,
        command_allowlist: Vec<String>,
    ) -> Self {
        Self {
            guest_root,
            events,
            command_allowlist,
            environments: HashMap::new(),
            guests: HashMap::new(),
            downloads: Arc::new(Mutex::new(DownloadRegistry::default())),
            next_guest: 0,
        }
    }

    pub(crate) fn create(
        &mut self,
        inner: &Arc<WebView2ProcessInner>,
        options: GuestCreateOptions,
    ) -> WebView2Result<GuestInfo> {
        let id = match options.id.clone() {
            Some(id) => id,
            None => {
                self.next_guest += 1;
                format!("guest-{}", self.next_guest)
            }
        };
        if self.guests.contains_key(&id) {
            self.destroy(&id)?;
        }
        let bounds = options.bounds.normalized();
        let partition = options
            .partition
            .clone()
            .filter(|partition| !partition.trim().is_empty())
            .unwrap_or_else(|| default_partition_for(&id));

        let parent = inner.hwnd.load(Ordering::Relaxed);
        let bounds = guest_host::physical_bounds(parent, bounds);
        let guest = self.build_guest(inner, &id, &partition, &options, bounds)?;
        let info = guest.info(&id);
        self.guests.insert(id, guest);
        self.sync_primary_holes(inner);
        guest_events::emit_guest_event(&self.events, "guest.created", info.to_json());
        Ok(info)
    }

    fn build_guest(
        &mut self,
        inner: &Arc<WebView2ProcessInner>,
        id: &str,
        partition: &str,
        options: &GuestCreateOptions,
        bounds: GuestBounds,
    ) -> WebView2Result<Guest> {
        let environment = self.environment(partition)?;
        let parent = inner.hwnd.load(Ordering::Relaxed);
        let (controller, surface) = self.create_surface(inner, &environment, parent, bounds)?;
        let webview = unsafe { controller.CoreWebView2() }.map_err(bridge::webview2_error)?;

        unsafe {
            controller
                .SetBounds(client_rect(bounds))
                .map_err(bridge::webview2_error)?;
            controller
                .SetIsVisible(options.visible)
                .map_err(bridge::webview2_error)?;
        }
        match &surface {
            GuestSurface::Windowed { hwnd } => {
                guest_host::raise_guest_above_primary(
                    *hwnd,
                    inner.primary_host.load(Ordering::Relaxed),
                );
                guest_host::set_host_window_visible(*hwnd, options.visible);
            }
            GuestSurface::Composition { .. } => {}
        }
        if let Some(color) = options
            .background_color
            .as_deref()
            .and_then(parse_background_color)
            && let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>()
        {
            let _ = unsafe { controller2.SetDefaultBackgroundColor(color) };
        }
        if options.allow_bridge {
            bridge::install_bridge_script_for(&webview, &self.command_allowlist)?;
            bridge::register_navigation_starting(&webview, inner.clone())?;
            bridge::register_web_message_received(&webview, inner.clone())?;
        }

        let state = Arc::new(Mutex::new(GuestState {
            url: options.url.clone().unwrap_or_default(),
            title: String::new(),
            bounds,
            visible: options.visible,
            loading: false,
            can_go_back: false,
            can_go_forward: false,
            zoom_factor: 1.0,
        }));
        guest_events::register_guest_handlers(
            &webview,
            GuestEventContext {
                id: id.to_string(),
                state: state.clone(),
                events: self.events.clone(),
                downloads: self.downloads.clone(),
                popup_policy: options.popup_policy,
                allow_downloads: options.allow_downloads,
            },
        )?;
        navigate_webview(&webview, options.url.as_deref(), options.html.as_deref())?;

        Ok(Guest {
            controller,
            webview,
            surface,
            state,
            partition: partition.to_string(),
            allow_bridge: options.allow_bridge,
            popup_policy: options.popup_policy,
        })
    }

    fn create_surface(
        &self,
        inner: &Arc<WebView2ProcessInner>,
        environment: &ICoreWebView2Environment,
        parent: isize,
        bounds: GuestBounds,
    ) -> WebView2Result<(ICoreWebView2Controller, GuestSurface)> {
        let mut dcomp = inner.dcomp.lock().unwrap();
        if let Some(root) = dcomp.as_mut() {
            let visual = composition::create_guest_visual(root, bounds)?;
            match composition::create_composition_controller(parent, environment, root, &visual) {
                Ok((composition_controller, controller)) => {
                    return Ok((
                        controller,
                        GuestSurface::Composition {
                            visual,
                            composition: composition_controller,
                        },
                    ));
                }
                Err(error) => {
                    let _ = composition::remove_guest_visual(root, &visual);
                    eprintln!(
                        "fenestra: composition guest failed, falling back to windowed: {error}"
                    );
                }
            }
        }
        let hwnd = guest_host::create_host_window(parent, bounds, true)?;
        match guest_host::create_controller(environment, hwnd) {
            Ok(controller) => Ok((controller, GuestSurface::Windowed { hwnd })),
            Err(error) => {
                guest_host::destroy_host_window(hwnd);
                Err(error)
            }
        }
    }

    pub(crate) fn destroy(&mut self, id: &str) -> WebView2Result<()> {
        let Some(guest) = self.guests.remove(id) else {
            // Idempotent: cleanup/races often destroy twice (React StrictMode).
            return Ok(());
        };
        if let Ok(mut downloads) = self.downloads.lock() {
            downloads.forget_guest(id);
        }
        let _ = unsafe { guest.controller.Close() };
        // Surface cleanup needs the process inner; callers pass via sync after.
        // Windowed path can destroy immediately; composition visuals are removed
        // when sync_primary_holes / explicit dispose runs with inner.
        if let GuestSurface::Windowed { hwnd } = guest.surface {
            guest_host::destroy_host_window(hwnd);
        }
        // Composition visuals leak until shutdown if we don't have inner here —
        // destroy is called with manager only. Store visual drop via Close which
        // is enough for WebView2; visual Remove happens in shutdown with inner.
        guest_events::emit_guest_event(&self.events, "guest.destroyed", json!({ "id": id }));
        Ok(())
    }

    pub(crate) fn destroy_with_inner(
        &mut self,
        inner: &Arc<WebView2ProcessInner>,
        id: &str,
    ) -> WebView2Result<()> {
        let Some(guest) = self.guests.remove(id) else {
            return Ok(());
        };
        if let Ok(mut downloads) = self.downloads.lock() {
            downloads.forget_guest(id);
        }
        let _ = unsafe { guest.controller.Close() };
        guest.dispose_surface(inner);
        self.sync_primary_holes(inner);
        guest_events::emit_guest_event(&self.events, "guest.destroyed", json!({ "id": id }));
        Ok(())
    }

    /// Close every guest. Called while the window shuts down, before the
    /// primary controller goes away.
    pub(crate) fn shutdown(&mut self, inner: &Arc<WebView2ProcessInner>) {
        if let Ok(mut downloads) = self.downloads.lock() {
            downloads.clear();
        }
        for (_, guest) in self.guests.drain() {
            let _ = unsafe { guest.controller.Close() };
            guest.dispose_surface(inner);
        }
        self.environments.clear();
        composition::clear_primary_holes(inner.primary_host.load(Ordering::Relaxed));
    }

    pub(crate) fn navigate(&self, id: &str, url: &str) -> WebView2Result<()> {
        navigate_webview(&self.guest(id)?.webview, Some(url), None)
    }

    pub(crate) fn set_bounds(
        &self,
        inner: &Arc<WebView2ProcessInner>,
        id: &str,
        bounds: GuestBounds,
    ) -> WebView2Result<()> {
        let guest = self.guest(id)?;
        let parent = inner.hwnd.load(Ordering::Relaxed);
        let bounds = guest_host::physical_bounds(parent, bounds.normalized());
        match &guest.surface {
            GuestSurface::Windowed { hwnd } => {
                guest_host::move_host_window(*hwnd, bounds);
                guest_host::raise_host_window(*hwnd);
            }
            GuestSurface::Composition { visual, .. } => {
                if let Ok(guard) = inner.dcomp.lock()
                    && let Some(dcomp) = guard.as_ref()
                {
                    composition::move_guest_visual(dcomp, visual, bounds)?;
                }
            }
        }
        unsafe { guest.controller.SetBounds(client_rect(bounds)) }
            .map_err(bridge::webview2_error)?;
        if let Ok(mut state) = guest.state.lock() {
            state.bounds = bounds;
        }
        self.sync_primary_holes(inner);
        Ok(())
    }

    pub(crate) fn raise_above_primary(&self, id: &str, primary_host: isize) -> WebView2Result<()> {
        let guest = self.guest(id)?;
        if let GuestSurface::Windowed { hwnd } = guest.surface {
            guest_host::raise_guest_above_primary(hwnd, primary_host);
        }
        Ok(())
    }

    pub(crate) fn raise_all(&self, primary_host: isize) {
        guest_host::lower_host_window(primary_host);
        for guest in self.guests.values() {
            if let GuestSurface::Windowed { hwnd } = guest.surface {
                guest_host::raise_host_window(hwnd);
            }
        }
    }

    pub(crate) fn set_visible(
        &self,
        inner: &Arc<WebView2ProcessInner>,
        id: &str,
        visible: bool,
    ) -> WebView2Result<()> {
        let guest = self.guest(id)?;
        if let GuestSurface::Windowed { hwnd } = guest.surface {
            guest_host::set_host_window_visible(hwnd, visible);
        }
        unsafe { guest.controller.SetIsVisible(visible) }.map_err(bridge::webview2_error)?;
        if let Ok(mut state) = guest.state.lock() {
            state.visible = visible;
        }
        self.sync_primary_holes(inner);
        Ok(())
    }

    pub(crate) fn set_covered(&self, inner: &Arc<WebView2ProcessInner>, covered: bool) {
        inner
            .guests_covered
            .store(covered, Ordering::Relaxed);
        self.sync_primary_holes(inner);
    }

    pub(crate) fn sync_primary_holes(&self, inner: &Arc<WebView2ProcessInner>) {
        let primary = inner.primary_host.load(Ordering::Relaxed);
        if primary == 0 {
            return;
        }
        if inner.guests_covered.load(Ordering::Relaxed) {
            composition::clear_primary_holes(primary);
            return;
        }
        let mut holes = Vec::new();
        for guest in self.guests.values() {
            if !matches!(guest.surface, GuestSurface::Composition { .. }) {
                continue;
            }
            let Ok(state) = guest.state.lock() else {
                continue;
            };
            if state.visible {
                holes.push(state.bounds);
            }
        }
        if holes.is_empty() {
            composition::clear_primary_holes(primary);
        } else {
            composition::set_primary_holes(primary, &holes);
        }
    }

    pub(crate) fn composition_hit_test(
        &self,
        point: (i32, i32),
    ) -> Option<composition::CompositionHit> {
        let mut hit = None;
        for guest in self.guests.values() {
            let GuestSurface::Composition {
                composition: comp, ..
            } = &guest.surface
            else {
                continue;
            };
            let Ok(state) = guest.state.lock() else {
                continue;
            };
            if !state.visible {
                continue;
            }
            let b = state.bounds;
            if point.0 >= b.x
                && point.1 >= b.y
                && point.0 < b.x + b.width as i32
                && point.1 < b.y + b.height as i32
            {
                hit = Some(composition::CompositionHit {
                    composition: comp.clone(),
                    controller: guest.controller.clone(),
                    bounds: b,
                });
            }
        }
        hit
    }

    pub(crate) fn focus(&self, id: &str) -> WebView2Result<()> {
        let guest = self.guest(id)?;
        unsafe {
            guest
                .controller
                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
        }
        .map_err(bridge::webview2_error)
    }

    pub(crate) fn reload(&self, id: &str, ignore_cache: bool) -> WebView2Result<()> {
        let guest = self.guest(id)?;
        if ignore_cache {
            // `ICoreWebView2::Reload` always reuses the cache; only the
            // DevTools protocol exposes a hard reload.
            return call_devtools(&guest.webview, "Page.reload", r#"{"ignoreCache":true}"#);
        }
        unsafe { guest.webview.Reload() }.map_err(bridge::webview2_error)
    }

    pub(crate) fn go_back(&self, id: &str) -> WebView2Result<()> {
        unsafe { self.guest(id)?.webview.GoBack() }.map_err(bridge::webview2_error)
    }

    pub(crate) fn go_forward(&self, id: &str) -> WebView2Result<()> {
        unsafe { self.guest(id)?.webview.GoForward() }.map_err(bridge::webview2_error)
    }

    pub(crate) fn set_zoom(&self, id: &str, factor: f64) -> WebView2Result<()> {
        let factor = factor.clamp(0.25, 5.0);
        let guest = self.guest(id)?;
        unsafe { guest.controller.SetZoomFactor(factor) }.map_err(bridge::webview2_error)?;
        if let Ok(mut state) = guest.state.lock() {
            state.zoom_factor = factor;
        }
        Ok(())
    }

    /// Snapshot the guest's current pixels as a `data:image/png;base64,...` URL.
    /// Call this before `setCovered(true)` so HTML overlays can dim over the page.
    pub(crate) fn capture_preview(&self, id: &str) -> WebView2Result<String> {
        use webview2_com::{
            CapturePreviewCompletedHandler,
            Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
        };
        use windows::Win32::System::{
            Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream},
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        };

        let guest = self.guest(id)?;
        let stream = unsafe {
            CreateStreamOnHGlobal(windows::Win32::Foundation::HGLOBAL::default(), true)
        }
        .map_err(|error| WebView2Error::Backend(format!("CreateStreamOnHGlobal: {error}")))?;
        let (tx, rx) = std::sync::mpsc::channel();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |status| {
            let _ = tx.send(status);
            Ok(())
        }));
        unsafe {
            guest
                .webview
                .CapturePreview(
                    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                    &stream,
                    &handler,
                )
        }
        .map_err(bridge::webview2_error)?;
        guest_host::wait_bounded(rx, "guest capturePreview", guest_host::SCRIPT_TIMEOUT)?
            .map_err(|error| {
                WebView2Error::Backend(format!("guest capturePreview failed: {error}"))
            })?;

        let hglobal = unsafe { GetHGlobalFromStream(&stream) }.map_err(|error| {
            WebView2Error::Backend(format!("GetHGlobalFromStream: {error}"))
        })?;
        let size = unsafe { GlobalSize(hglobal) };
        if size == 0 {
            return Err(WebView2Error::Backend(
                "guest capturePreview produced an empty image".into(),
            ));
        }
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            return Err(WebView2Error::Backend(
                "guest capturePreview GlobalLock failed".into(),
            ));
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }.to_vec();
        unsafe {
            let _ = GlobalUnlock(hglobal);
        }
        Ok(format!(
            "data:image/png;base64,{}",
            encode_base64(&bytes)
        ))
    }

    pub(crate) fn execute_javascript(&self, id: &str, code: &str) -> WebView2Result<Value> {
        let guest = self.guest(id)?;
        let (tx, rx) = std::sync::mpsc::channel();
        let handler = ExecuteScriptCompletedHandler::create(Box::new(move |status, result| {
            let _ = tx.send(status.map(|()| result));
            Ok(())
        }));
        let wide = bridge::wide_pwstr(code);
        unsafe {
            guest
                .webview
                .ExecuteScript(windows::core::PCWSTR(wide.as_ptr()), &handler)
        }
        .map_err(bridge::webview2_error)?;
        let raw = guest_host::wait_bounded(rx, "guest executeJavaScript", guest_host::SCRIPT_TIMEOUT)?
            .map_err(|error| {
                WebView2Error::Backend(format!("guest executeJavaScript failed: {error}"))
            })?;
        Ok(serde_json::from_str(&raw).unwrap_or(Value::Null))
    }

    pub(crate) fn info(&self, id: &str) -> WebView2Result<GuestInfo> {
        self.guests
            .get(id)
            .map(|guest| guest.info(id))
            .ok_or_else(|| missing_guest(id))
    }

    pub(crate) fn list(&self) -> Vec<GuestInfo> {
        let mut guests: Vec<GuestInfo> = self
            .guests
            .iter()
            .map(|(id, guest)| guest.info(id))
            .collect();
        guests.sort_by(|left, right| left.id.cmp(&right.id));
        guests
    }

    pub(crate) fn partition_of(&self, id: &str) -> Option<String> {
        self.guests.get(id).map(|guest| guest.partition.clone())
    }

    pub(crate) fn download_action(
        &self,
        download_id: &str,
        action: GuestDownloadAction,
        save_path: Option<&str>,
    ) -> WebView2Result<()> {
        let Ok(mut downloads) = self.downloads.lock() else {
            return Err(WebView2Error::Backend(
                "guest download registry is unavailable".to_string(),
            ));
        };
        downloads.apply(download_id, action, save_path)
    }

    fn guest(&self, id: &str) -> WebView2Result<&Guest> {
        self.guests.get(id).ok_or_else(|| missing_guest(id))
    }

    fn environment(&mut self, partition: &str) -> WebView2Result<ICoreWebView2Environment> {
        if let Some(environment) = self.environments.get(partition) {
            return Ok(environment.clone());
        }
        let folder = self
            .guest_root
            .join(format!("{:016x}", stable_hash(&[partition])));
        let environment = guest_host::create_environment(&folder)?;
        self.environments
            .insert(partition.to_string(), environment.clone());
        Ok(environment)
    }
}

fn navigate_webview(
    webview: &ICoreWebView2,
    url: Option<&str>,
    html: Option<&str>,
) -> WebView2Result<()> {
    if let Some(url) = url.map(str::trim).filter(|url| !url.is_empty()) {
        let wide = bridge::wide_pwstr(url);
        return unsafe { webview.Navigate(windows::core::PCWSTR(wide.as_ptr())) }
            .map_err(bridge::webview2_error);
    }
    if let Some(html) = html.filter(|html| !html.is_empty()) {
        let wide = bridge::wide_pwstr(html);
        return unsafe { webview.NavigateToString(windows::core::PCWSTR(wide.as_ptr())) }
            .map_err(bridge::webview2_error);
    }
    Err(WebView2Error::Backend(
        "guest navigation needs a `url` or `html`".to_string(),
    ))
}

fn call_devtools(webview: &ICoreWebView2, method: &str, parameters: &str) -> WebView2Result<()> {
    let method_wide = bridge::wide_pwstr(method);
    let params_wide = bridge::wide_pwstr(parameters);
    let handler = webview2_com::CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
        |_status, _result| Ok(()),
    ));
    unsafe {
        webview.CallDevToolsProtocolMethod(
            windows::core::PCWSTR(method_wide.as_ptr()),
            windows::core::PCWSTR(params_wide.as_ptr()),
            &handler,
        )
    }
    .map_err(bridge::webview2_error)
}

/// Guest controllers fill their host window, so the controller rect is
/// always the child client area.
fn client_rect(bounds: GuestBounds) -> windows::Win32::Foundation::RECT {
    windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: bounds.width.max(1) as i32,
        bottom: bounds.height.max(1) as i32,
    }
}

fn parse_background_color(value: &str) -> Option<COREWEBVIEW2_COLOR> {
    let hex = value.trim().trim_start_matches('#');
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(hex.get(range)?, 16).ok();
    match hex.len() {
        6 => Some(COREWEBVIEW2_COLOR {
            A: 255,
            R: channel(0..2)?,
            G: channel(2..4)?,
            B: channel(4..6)?,
        }),
        8 => Some(COREWEBVIEW2_COLOR {
            A: channel(0..2)?,
            R: channel(2..4)?,
            G: channel(4..6)?,
            B: channel(6..8)?,
        }),
        _ => None,
    }
}

fn missing_guest(id: &str) -> WebView2Error {
    WebView2Error::Backend(format!("unknown guest: {id}"))
}

fn encode_base64(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_background_color() {
        let color = parse_background_color("#102030").expect("color");
        assert_eq!((color.A, color.R, color.G, color.B), (255, 16, 32, 48));
    }

    #[test]
    fn parses_argb_background_color() {
        let color = parse_background_color("80102030").expect("color");
        assert_eq!((color.A, color.R, color.G, color.B), (128, 16, 32, 48));
    }

    #[test]
    fn rejects_short_background_color() {
        assert!(parse_background_color("#fff").is_none());
    }

    #[test]
    fn guest_bounds_fill_the_child_client_area() {
        let rect = client_rect(GuestBounds::new(40, 80, 320, 240));
        assert_eq!((rect.left, rect.top, rect.right, rect.bottom), (0, 0, 320, 240));
    }
}
