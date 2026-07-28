// WebView2 launch flow — Windows-only.
//
// The launch flow on a real Windows host is:
//
// 1. Build a winit `EventLoop` and create a `Window` (frameless if the
//    user asked for one). Extract the `HWND` via `raw_window_handle`.
// 2. Apply DWM backdrop / glass if the user asked for a glass effect
//    (see `host_controls::apply_dwm_backdrop`).
// 3. Create a per-window `WebView2` user data directory under
//    `%LOCALAPPDATA%/fenestra/webviews/<stable hash>/<instance>`.
// 4. Drive the COM-style async env creation: build a
//    `CreateCoreWebView2EnvironmentCompletedHandler` whose callback
//    sends the result to an `mpsc::channel`, kick off
//    `CreateCoreWebView2EnvironmentWithOptions`, and call
//    `webview2_com::wait_with_pump` on the channel. This blocks the
//    UI thread but pumps Win32 messages, which is exactly what the
//    COM apartment expects.
// 5. From the env, do the same dance with
//    `CreateCoreWebView2ControllerCompletedHandler` to obtain a
//    `ICoreWebView2Controller`.
// 6. Wire up the event handlers on the controller's `ICoreWebView2`:
//    - `add_NavigationStarting` — intercept `fenestra://bridge/...`
//      and `fenestra://window/...` URLs.
//    - `add_WebMessageReceived` — receive `postMessage(...)` from the
//      page so plain-text window commands work without a navigation.
//    - `AddScriptToExecuteOnDocumentCreated` — install the canonical
//      Fenestra bridge script (`fenestra_bridge::install_script`)
//      into every main-frame document.
// 7. If the entry URL is `http(s)://`, probe the dev server with
//    short TCP connect timeouts before `Navigate` (so a Vite-style
//    dev server has a chance to start).
// 8. `webview.Navigate(url)`. WebView2 repaints itself; winit only
//    drives window events.
// 9. Run the winit event loop. Window and WebView2 creation happen in
//    `ApplicationHandler::can_create_surfaces` — winit 0.31 does not call
//    `resumed` on Windows. The loop processes `winit::Event`s plus user
//    plus user events delivered via an `mpsc` channel
//    (`WebView2UserEvent`) that the app's `about_to_wait` callback
//    drains. The winit 0.31 API does not support typed user events
//    on the `EventLoopProxy` (only a bare `wake_up`), so the channel
//    is the only safe way to talk to the UI thread from a bridge
//    handler or activity emitter running on a worker thread.

use std::{
    net::ToSocketAddrs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

use fenestra_bridge::{ActivityHostUpdate, LaunchMetrics};
use fenestra_runtime::RuntimeInfo;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes, WindowId, WindowLevel},
};

use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_COLOR, ICoreWebView2Controller, ICoreWebView2Controller2,
        ICoreWebView2Environment, ICoreWebView2EnvironmentOptions,
    },
};
use webview2_com_sys::Microsoft::Web::WebView2::Win32 as SysWin32;
use windows::core::Interface;

use crate::{
    WebView2Config, WebView2Error, WebView2Process, WebView2ProcessInner, WebView2Result,
    WebView2Window,
    windows::{
        bridge, composition, desktop_services, guest::GuestManager, guest_commands, guest_host,
        regions,
    },
};

pub(crate) fn launch(
    window: WebView2Window,
    runtime: RuntimeInfo,
) -> WebView2Result<WebView2Process> {
    let metrics = LaunchMetrics::new(metrics_label(&window.config));
    metrics.mark("launch.start");

    // Avoid the default opaque white underlay before the controller applies
    // SetDefaultBackgroundColor. Format is 0xAARRGGBB.
    if window.config.transparent
        || window
            .config
            .effective_background_effect()
            .requires_transparency()
    {
        unsafe {
            std::env::set_var("WEBVIEW2_DEFAULT_BACKGROUND_COLOR", "0x00000000");
        }
    } else {
        unsafe {
            std::env::set_var("WEBVIEW2_DEFAULT_BACKGROUND_COLOR", "0xFF0A0A0A");
        }
    }

    let url = entry_url(&window.config)?;
    metrics.mark("entry_url.ready");

    let event_loop = EventLoop::new()
        .map_err(|error| WebView2Error::Backend(format!("winit event loop: {error}")))?;
    metrics.mark("event_loop.ready");

    let (event_tx, event_rx) = std::sync::mpsc::channel::<WebView2UserEvent>();

    let bridge_runtime = fenestra_bridge::BridgeRuntime::new(
        window.bridge_handlers.clone(),
        window.config.bridge.clone(),
        window.config.security.clone(),
    );
    metrics.mark("bridge_runtime.ready");

    let sidecar = spawn_dev_command(window.config.dev_command.as_deref());
    if sidecar.is_some() {
        metrics.mark("dev_command.started");
    }

    let activity = fenestra_bridge::ActivityRegistry::default();
    let command_allowlist =
        fenestra_bridge::bridge_commands_with_all_internal(window.config.bridge.commands());

    let emitter = Arc::new(crate::WebView2ActivityEmitter {
        sender: event_tx.clone(),
        activity: activity.clone(),
    });

    let inner = {
        #[allow(clippy::arc_with_non_send_sync)]
        Arc::new(WebView2ProcessInner {
            hwnd: std::sync::atomic::AtomicIsize::new(0),
            primary_host: std::sync::atomic::AtomicIsize::new(0),
            controller: Mutex::new(None),
            webview: Mutex::new(None),
            bridge_runtime: Mutex::new(Some(bridge_runtime)),
            activity: activity.clone(),
            emitter: emitter.clone(),
            metrics: metrics.clone(),
            event_sender: event_tx.clone(),
            runtime: runtime.clone(),
            background_frame_rate: window.config.lifecycle.background_frame_rate,
            command_allowlist: command_allowlist.clone(),
            hide_on_blur: std::sync::atomic::AtomicBool::new(window.config.hide_on_blur),
            desktop_services: Mutex::new(None),
            guests: Mutex::new(GuestManager::new(
                guest_user_data_root(&window.config),
                event_tx.clone(),
                command_allowlist.clone(),
            )),
            wake: Mutex::new(None),
            dcomp: Mutex::new(None),
            primary_composition: Mutex::new(None),
            guests_covered: std::sync::atomic::AtomicBool::new(false),
        })
    };

    let proxy = event_loop.create_proxy();
    *inner.wake.lock().unwrap() = Some(proxy);

    let desktop = desktop_services::apply_windows_desktop_services(
        window.config.desktop_services.tray_icon.as_ref(),
        &window.config.desktop_services.autostart,
        &window.config.desktop_services.global_shortcuts,
        &window.config.desktop_services.deep_links,
        &window.config.desktop_services.native_messaging_hosts,
        window.config.desktop_services.single_instance_id.as_deref(),
        window.config.desktop_services.single_instance_policy,
    )
    .map_err(WebView2Error::Backend)?;
    *inner.desktop_services.lock().unwrap() = Some(desktop);
    metrics.mark("desktop_services.ready");

    let state = LaunchState {
        config: window.config,
        url,
        inner: inner.clone(),
        event_rx,
        window: None,
        sidecar,
        frameless_restore: None,
    };
    let app = LaunchApp { state };
    event_loop
        .run_app(app)
        .map_err(|error| WebView2Error::Backend(format!("winit run_app: {error}")))?;
    metrics.mark("event_loop.exit");

    Ok(WebView2Process { inner })
}

struct LaunchState {
    config: WebView2Config,
    url: String,
    inner: Arc<WebView2ProcessInner>,
    event_rx: Receiver<WebView2UserEvent>,
    window: Option<Box<dyn Window>>,
    sidecar: Option<std::process::Child>,
    /// Previous outer rect while a borderless window is filled to the
    /// monitor work area (not Win32 `SW_MAXIMIZE`).
    frameless_restore: Option<windows::Win32::Foundation::RECT>,
}

struct LaunchApp {
    state: LaunchState,
}

impl ApplicationHandler for LaunchApp {
    fn new_events(&mut self, event_loop: &dyn ActiveEventLoop, _cause: winit::event::StartCause) {
        self.drain_user_events(event_loop);
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self
            .state
            .inner
            .hwnd
            .load(std::sync::atomic::Ordering::Relaxed)
            != 0
        {
            return;
        }
        let dwm_glass = super::host_controls::wants_dwm_glass(&self.state.config);
        let winit_transparent = self.state.config.transparent && !dwm_glass;
        // Always create hidden; show only after Navigate so the user never
        // stares at a blank/white HWND while WebView2 + Vite come up.
        let mut attributes = WindowAttributes::default()
            .with_title(self.state.config.title.clone())
            .with_surface_size(PhysicalSize::new(
                self.state.config.width.max(1) as f64,
                self.state.config.height.max(1) as f64,
            ))
            .with_visible(false)
            .with_resizable(self.state.config.resizable)
            .with_min_surface_size(PhysicalSize::new(
                self.state.config.min_width.max(1) as f64,
                self.state.config.min_height.max(1) as f64,
            ))
            .with_decorations(self.state.config.chrome.uses_native_decorations())
            .with_transparent(winit_transparent);
        if self.state.config.always_on_top {
            attributes = attributes.with_window_level(WindowLevel::AlwaysOnTop);
        }
        let window: Box<dyn Window> = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("fenestra: failed to create winit window: {error}");
                event_loop.exit();
                return;
            }
        };
        let hwnd = match window.window_handle() {
            Ok(handle) => match handle.as_raw() {
                RawWindowHandle::Win32(handle) => handle.hwnd.get(),
                _ => {
                    eprintln!("fenestra: winit did not return a Win32 handle");
                    event_loop.exit();
                    return;
                }
            },
            Err(error) => {
                eprintln!("fenestra: failed to extract HWND: {error}");
                event_loop.exit();
                return;
            }
        };
        self.state
            .inner
            .hwnd
            .store(hwnd, std::sync::atomic::Ordering::Relaxed);
        self.state.inner.metrics.mark("hwnd.ready");
        // Keep the winit window alive before we pump COM messages.
        self.state.window = Some(window);

        if !self.state.config.chrome.uses_native_decorations() {
            super::host_controls::apply_frameless_window(hwnd);
        }

        // Do NOT apply DWM glass / window-vibrancy before the WebView2
        // controller exists — that returns E_INVALIDARG (0x80070057) from
        // CreateCoreWebView2Controller on current WebView2 runtimes.
        match create_webview2(
            hwnd,
            &self.state.config,
            &self.state.url,
            self.state.inner.clone(),
        ) {
            Ok(()) => self.state.inner.metrics.mark("controller.ready"),
            Err(error) => {
                eprintln!("fenestra: WebView2 controller failed: {error}");
                event_loop.exit();
                return;
            }
        }

        // WebView2 can restore caption chrome; re-assert frameless after.
        if !self.state.config.chrome.uses_native_decorations() {
            super::host_controls::apply_frameless_window(hwnd);
        }
        let _ = super::host_controls::apply_dwm_backdrop(hwnd, &self.state.config);
        let _ = super::host_controls::apply_window_vibrancy(hwnd, &self.state.config);
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        self.drain_user_events(event_loop);
        match event {
            WindowEvent::CloseRequested => {
                let _ = self.state.inner.event_sender.send(WebView2UserEvent::Exit);
            }
            WindowEvent::SurfaceResized(size) => {
                let hwnd = self
                    .state
                    .inner
                    .hwnd
                    .load(std::sync::atomic::Ordering::Relaxed);
                let frameless = !self.state.config.chrome.uses_native_decorations();
                let work_area_maximized = false;
                resize_controller(
                    &self.state.inner,
                    size.width,
                    size.height,
                    frameless,
                    hwnd,
                    work_area_maximized,
                );
                if frameless {
                    // Keep DWM caption suppressed after native maximize/snap.
                    // Never convert IsZoomed into a fake SetWindowPos fill.
                    let _ = super::host_controls::suppress_system_maximize(hwnd);
                }
                if super::host_controls::wants_dwm_glass(&self.state.config) {
                    let _ = super::host_controls::apply_dwm_backdrop(hwnd, &self.state.config);
                }
            }
            WindowEvent::Focused(false)
                if self
                    .state
                    .inner
                    .hide_on_blur
                    .load(std::sync::atomic::Ordering::Relaxed) =>
            {
                let hwnd = self
                    .state
                    .inner
                    .hwnd
                    .load(std::sync::atomic::Ordering::Relaxed);
                let _ = super::host_controls::hide_window(hwnd);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        let platform_events = match self.state.inner.desktop_services.lock() {
            Ok(guard) => {
                if let Some(services) = guard.as_ref() {
                    services.poll_native_events();
                    services.take_events()
                } else {
                    Vec::new()
                }
            }
            Err(_) => Vec::new(),
        };
        for event in platform_events {
            self.dispatch_platform_event(event);
        }
        self.drain_user_events(event_loop);
    }
}

impl LaunchApp {
    fn drain_user_events(&mut self, event_loop: &dyn ActiveEventLoop) {
        loop {
            match self.state.event_rx.try_recv() {
                Ok(event) => self.handle_user_event(event_loop, event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    event_loop.exit();
                    break;
                }
            }
        }
    }

    fn dispatch_platform_event(&mut self, event: fenestra_platform::PlatformEvent) {
        if let fenestra_platform::PlatformEvent::SingleInstance(activation) = &event
            && activation.policy == fenestra_platform::SingleInstancePolicy::FocusExisting
        {
            let hwnd = self
                .state
                .inner
                .hwnd
                .load(std::sync::atomic::Ordering::Relaxed);
            let _ = super::host_controls::show_window(hwnd);
            let _ = super::host_controls::focus_window(hwnd);
        }
        let (name, payload) = platform_event_payload(event);
        if let Some(webview) = self.state.inner.webview.lock().unwrap().clone() {
            bridge::execute_bridge_emit(&webview, name, &payload);
        }
    }

    fn handle_user_event(&mut self, event_loop: &dyn ActiveEventLoop, event: WebView2UserEvent) {
        let hwnd = self
            .state
            .inner
            .hwnd
            .load(std::sync::atomic::Ordering::Relaxed);
        match event {
            WebView2UserEvent::BridgeEvent { name, payload } => {
                if let Some(webview) = self.state.inner.webview.lock().unwrap().clone() {
                    bridge::execute_bridge_emit(&webview, &name, &payload);
                }
            }
            WebView2UserEvent::Activity { update } => {
                emit_activity_event(self.state.inner.clone(), update);
            }
            WebView2UserEvent::GuestOpenRequested { parent, url } => {
                guest_commands::open_requested_guest(&self.state.inner, &parent, &url);
            }
            WebView2UserEvent::GuestBridge {
                request_id,
                command,
            } => {
                bridge::complete_guest_bridge(&self.state.inner, &request_id, command);
            }
            WebView2UserEvent::SetVisible(visible) => {
                if visible {
                    let _ = super::host_controls::show_window(hwnd);
                } else {
                    let _ = super::host_controls::hide_window(hwnd);
                }
            }
            WebView2UserEvent::Show => {
                let _ = super::host_controls::show_window(hwnd);
            }
            WebView2UserEvent::Hide => {
                let _ = super::host_controls::hide_window(hwnd);
            }
            WebView2UserEvent::Focus => {
                let _ = super::host_controls::focus_window(hwnd);
            }
            WebView2UserEvent::Minimize => {
                let _ = super::host_controls::minimize_window(hwnd);
            }
            WebView2UserEvent::Maximize => {
                let _ = super::host_controls::toggle_maximize_window(hwnd);
            }
            WebView2UserEvent::Unmaximize => {
                let _ = super::host_controls::unmaximize_window(hwnd);
            }
            WebView2UserEvent::Exit => {
                // Hide first so teardown does not flash a blank/white surface.
                let _ = super::host_controls::hide_window(hwnd);
                if let Some(mut child) = self.state.sidecar.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                if let Ok(mut guests) = self.state.inner.guests.lock() {
                    guests.shutdown(&self.state.inner);
                }
                composition::detach_input_subclass(hwnd);
                *self.state.inner.primary_composition.lock().unwrap() = None;
                *self.state.inner.dcomp.lock().unwrap() = None;
                {
                    let mut webview = self.state.inner.webview.lock().unwrap();
                    *webview = None;
                }
                {
                    let mut controller = self.state.inner.controller.lock().unwrap();
                    if let Some(controller) = controller.take() {
                        let _ = unsafe { controller.Close() };
                    }
                }
                self.state.window = None;
                event_loop.exit();
            }
        }
    }
}

fn resize_controller(
    inner: &Arc<WebView2ProcessInner>,
    width: u32,
    height: u32,
    frameless: bool,
    hwnd: isize,
    work_area_maximized: bool,
) {
    if width == 0 || height == 0 {
        return;
    }
    let primary_host = inner
        .primary_host
        .load(std::sync::atomic::Ordering::Relaxed);
    if primary_host != 0 {
        guest_host::resize_primary_host_window(primary_host, hwnd);
    }
    let Ok(guard) = inner.controller.lock() else {
        return;
    };
    let Some(controller) = guard.as_ref() else {
        return;
    };
    let size =
        controller_bounds_for_size(width, height, frameless, hwnd, work_area_maximized);
    let _ = unsafe { controller.SetBounds(size) };
    drop(guard);
    if let Ok(manager) = inner.guests.try_lock() {
        manager.raise_all(primary_host);
        manager.sync_primary_holes(inner);
    }
}

fn create_webview2(
    hwnd: isize,
    config: &WebView2Config,
    url: &str,
    inner: Arc<WebView2ProcessInner>,
) -> WebView2Result<()> {
    if hwnd == 0 {
        return Err(WebView2Error::Backend(
            "invalid parent HWND for WebView2 controller: null".into(),
        ));
    }
    // Probe before creating the controller so a slow Vite handoff never
    // paints a frozen white window on the UI thread mid-setup.
    wait_for_dev_server(url, &inner.event_sender);

    match composition::DCompRoot::create(hwnd) {
        Ok(dcomp) => {
            *inner.dcomp.lock().unwrap() = Some(dcomp);
            composition::attach_input_subclass(hwnd, inner.clone());
        }
        Err(error) => {
            eprintln!("fenestra: DirectComposition unavailable, guests stay windowed: {error}");
        }
    }

    let user_data_dir = webview_user_data_dir(config);
    std::fs::create_dir_all(&user_data_dir).map_err(WebView2Error::Io)?;
    let user_data_dir_str = user_data_dir
        .to_str()
        .ok_or_else(|| WebView2Error::Backend("user data dir is not UTF-8".to_string()))?;
    let user_data_wide = bridge::wide_pwstr(user_data_dir_str);
    let options: ICoreWebView2EnvironmentOptions = CoreWebView2EnvironmentOptions::default().into();

    let env: ICoreWebView2Environment = {
        let (tx, rx) = std::sync::mpsc::channel();
        let env_options = options.clone();
        let user_data_ptr = user_data_wide.as_ptr();
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |error_code, environment| {
                let result = (|| {
                    error_code?;
                    environment.ok_or_else(|| {
                        windows::core::Error::from(windows::core::HRESULT(0x80004003u32 as i32))
                    })
                })();
                tx.send(result).map_err(|_| {
                    windows::core::Error::from(windows::core::HRESULT(0x80000004u32 as i32))
                })
            },
        ));
        unsafe {
            SysWin32::CreateCoreWebView2EnvironmentWithOptions(
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR(user_data_ptr),
                &env_options,
                &handler,
            )
            .map_err(|error| {
                WebView2Error::Backend(format!("CreateCoreWebView2EnvironmentWithOptions: {error}"))
            })?;
        }
        match webview2_com::wait_with_pump(rx) {
            Ok(Ok(env)) => env,
            Ok(Err(error)) => {
                return Err(WebView2Error::Backend(format!(
                    "CreateCoreWebView2Environment callback: {error}"
                )));
            }
            Err(error) => {
                return Err(WebView2Error::Backend(format!(
                    "env wait_with_pump: {error}"
                )));
            }
        }
    };
    inner.metrics.mark("env.ready");

    // Prefer dual composition: primary visual above guests so HTML overlays
    // (dialogs with bg-black/50) can alpha-blend over live guest content.
    let mut composition_primary = false;
    let controller: ICoreWebView2Controller = {
        let composed = match inner.dcomp.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(dcomp) => {
                    match composition::create_composition_controller(
                        hwnd,
                        &env,
                        &dcomp.primary_visual,
                    ) {
                        Ok(pair) => Some(pair),
                        Err(error) => {
                            eprintln!(
                                "fenestra: composition primary unavailable, falling back to HWND: {error}"
                            );
                            None
                        }
                    }
                }
                None => None,
            },
            Err(_) => None,
        };
        if let Some((composition_ctrl, controller)) = composed {
            *inner.primary_composition.lock().unwrap() = Some(composition_ctrl);
            composition_primary = true;
            controller
        } else {
            let primary_host = guest_host::create_primary_host_window(hwnd)?;
            inner
                .primary_host
                .store(primary_host, std::sync::atomic::Ordering::Relaxed);
            let parent = windows::Win32::Foundation::HWND(primary_host as *mut _);
            let (tx, rx) = std::sync::mpsc::channel();
            let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                move |error_code, controller| {
                    let result = (|| {
                        error_code?;
                        controller.ok_or_else(|| {
                            windows::core::Error::from(windows::core::HRESULT(0x80004003u32 as i32))
                        })
                    })();
                    tx.send(result).map_err(|_| {
                        windows::core::Error::from(windows::core::HRESULT(0x80000004u32 as i32))
                    })
                },
            ));
            unsafe {
                env.CreateCoreWebView2Controller(parent, &handler)
                    .map_err(|error| {
                        WebView2Error::Backend(format!("CreateCoreWebView2Controller: {error}"))
                    })?;
            }
            match webview2_com::wait_with_pump(rx) {
                Ok(Ok(controller)) => controller,
                Ok(Err(error)) => {
                    return Err(WebView2Error::Backend(format!(
                        "CreateCoreWebView2Controller callback: {error}"
                    )));
                }
                Err(error) => {
                    return Err(WebView2Error::Backend(format!(
                        "controller wait_with_pump: {error}"
                    )));
                }
            }
        }
    };
    inner.metrics.mark("controller.env.ready");

    // GetClientRect can return an empty rect before the first layout
    // pass; WebView2 SetBounds rejects 0×0 with E_INVALIDARG.
    let bounds_hwnd = if composition_primary {
        hwnd
    } else {
        inner
            .primary_host
            .load(std::sync::atomic::Ordering::Relaxed)
    };
    let size = controller_bounds(bounds_hwnd, config);
    unsafe {
        controller.SetBounds(size).map_err(|error| {
            WebView2Error::Backend(format!(
                "SetBounds({:?}): {error}",
                (size.left, size.top, size.right, size.bottom)
            ))
        })?;
        // Keep the controller hidden until Navigate — visible=true here is what
        // produced the white frozen window during startup.
        controller.SetIsVisible(false).map_err(|error| {
            WebView2Error::Backend(format!("SetIsVisible: {error}"))
        })?;
    }

    let needs_clear_bg = composition_primary
        || config.transparent
        || config.effective_background_effect().requires_transparency();
    if needs_clear_bg {
        set_webview_transparent_background(&controller);
    } else {
        set_webview_opaque_background(&controller, 0x0a, 0x0a, 0x0a);
    }

    let webview = unsafe { controller.CoreWebView2() }.map_err(|error| {
        WebView2Error::Backend(format!("CoreWebView2: {error}"))
    })?;
    inner.metrics.mark("webview.ready");

    if let Ok(settings) = unsafe { webview.Settings() } {
        let _ = unsafe { settings.SetAreDefaultContextMenusEnabled(false) };
        let _ = unsafe { settings.SetAreDevToolsEnabled(true) };
    }

    if config.frameless || !config.drag_regions.is_empty() || !config.control_regions.is_empty() {
        regions::enable_non_client_region_support(&webview).map_err(|error| {
            WebView2Error::Backend(format!("non-client region support: {error}"))
        })?;
    }

    if needs_clear_bg {
        install_transparent_document_script(&webview)?;
        register_transparent_background_on_navigation(&webview, controller.clone())?;
    }

    bridge::install_bridge_script(&webview, &inner)?;
    regions::install_region_script(&webview, config)?;
    bridge::register_navigation_starting(&webview, inner.clone())?;
    bridge::register_web_message_received(&webview, inner.clone())?;

    let url_wide = bridge::wide_pwstr(url);
    unsafe {
        webview
            .Navigate(windows::core::PCWSTR(url_wide.as_ptr()))
            .map_err(|error| WebView2Error::Backend(format!("Navigate({url}): {error}")))?;
    }
    inner.metrics.mark("navigate.ready");

    if config.visible {
        unsafe {
            let _ = controller.SetIsVisible(true);
        }
        let _ = super::host_controls::show_window(hwnd);
    }

    *inner.controller.lock().unwrap() = Some(controller);
    *inner.webview.lock().unwrap() = Some(webview);

    Ok(())
}

fn controller_bounds(
    hwnd: isize,
    config: &WebView2Config,
) -> windows::Win32::Foundation::RECT {
    let frameless = config.frameless || !config.chrome.uses_native_decorations();
    if let Some(rect) = super::host_controls::client_rect(hwnd) {
        let width = rect.right.saturating_sub(rect.left);
        let height = rect.bottom.saturating_sub(rect.top);
        if width > 0 && height > 0 {
            return controller_bounds_for_size(width as u32, height as u32, frameless, hwnd, false);
        }
    }
    controller_bounds_for_size(
        config.width.max(1),
        config.height.max(1),
        frameless,
        hwnd,
        false,
    )
}

fn controller_bounds_for_size(
    width: u32,
    height: u32,
    _frameless: bool,
    _hwnd: isize,
    _work_area_maximized: bool,
) -> windows::Win32::Foundation::RECT {
    // Keep WebView2 edge-to-edge. Frameless resize is handled by injected
    // edge hit strips (see regions.rs) that ask the host to begin an NC drag.
    windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: (width as i32).max(1),
        bottom: (height as i32).max(1),
    }
}

fn emit_activity_event(inner: Arc<WebView2ProcessInner>, update: ActivityHostUpdate) {
    let name = match update {
        ActivityHostUpdate::Begin(_) => "fenestra.activity.begin",
        ActivityHostUpdate::End(_) => "fenestra.activity.end",
    };
    let payload = fenestra_bridge::host_update_json(&update);
    if let Some(webview) = inner.webview.lock().unwrap().clone() {
        bridge::execute_bridge_emit(&webview, name, &payload);
    }
}

fn platform_event_payload(
    event: fenestra_platform::PlatformEvent,
) -> (&'static str, serde_json::Value) {
    match event {
        fenestra_platform::PlatformEvent::Tray(activation) => (
            "tray.activate",
            serde_json::json!({
                "trayId": activation.tray_id,
                "itemId": activation.item_id,
                "action": activation.action,
            }),
        ),
        fenestra_platform::PlatformEvent::GlobalShortcut(activation) => (
            "globalShortcut.activate",
            serde_json::json!({
                "id": activation.id,
                "action": activation.action,
                "activationToken": activation.activation_token,
            }),
        ),
        fenestra_platform::PlatformEvent::SingleInstance(activation) => (
            "singleInstance.activate",
            serde_json::json!({
                "policy": format!("{:?}", activation.policy),
                "arguments": activation.arguments,
                "workingDirectory": activation.working_directory,
                "activationToken": activation.activation_token,
            }),
        ),
    }
}

fn set_webview_transparent_background(
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
) {
    set_webview_background_color(controller, 0, 0, 0, 0);
}

fn set_webview_opaque_background(
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    r: u8,
    g: u8,
    b: u8,
) {
    set_webview_background_color(controller, 255, r, g, b);
}

fn set_webview_background_color(
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    a: u8,
    r: u8,
    g: u8,
    b: u8,
) {
    match controller.cast::<ICoreWebView2Controller2>() {
        Ok(controller2) => {
            let color = COREWEBVIEW2_COLOR {
                A: a,
                R: r,
                G: g,
                B: b,
            };
            if let Err(error) = unsafe { controller2.SetDefaultBackgroundColor(color) } {
                eprintln!("fenestra: WebView2 background color failed: {error}");
            }
        }
        Err(error) => {
            eprintln!("fenestra: ICoreWebView2Controller2 unavailable: {error}");
        }
    }
}

fn register_transparent_background_on_navigation(
    webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    controller: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
) -> WebView2Result<()> {
    use webview2_com::NavigationCompletedEventHandler;
    let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, _args| {
        set_webview_transparent_background(&controller);
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        webview
            .add_NavigationCompleted(&handler, &mut token)
            .map_err(bridge::webview2_error)?;
    }
    Ok(())
}

fn install_transparent_document_script(
    webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
) -> WebView2Result<()> {
    let script = r#"(function(){
  var css = 'html,body{background:transparent!important;}';
  var style = document.createElement('style');
  style.setAttribute('data-fenestra-transparent', '1');
  style.textContent = css;
  (document.head || document.documentElement).appendChild(style);
  if (document.documentElement) document.documentElement.style.background = 'transparent';
  if (document.body) document.body.style.background = 'transparent';
})();"#;
    let wide = bridge::wide_pwstr(script);
    let completed = webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(
        Box::new(|_error, _id| Ok(())),
    );
    unsafe {
        webview
            .AddScriptToExecuteOnDocumentCreated(windows::core::PCWSTR(wide.as_ptr()), &completed)
    }
    .map_err(bridge::webview2_error)?;
    Ok(())
}

fn webview_user_data_dir(config: &WebView2Config) -> PathBuf {
    profile_root(config).join("profile")
}

/// Guest partitions live next to the primary profile. Each partition
/// gets its own folder below this root so cookies and cache stay
/// separate.
fn guest_user_data_root(config: &WebView2Config) -> PathBuf {
    profile_root(config).join("guests")
}

fn profile_root(config: &WebView2Config) -> PathBuf {
    let profile_key = config
        .app_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(config.title.as_str());
    user_cache_home()
        .join("fenestra")
        .join("webviews")
        .join(format!("{:016x}", stable_hash(&[profile_key])))
}

fn spawn_dev_command(command: Option<&str>) -> Option<std::process::Child> {
    let command = command?.trim();
    if command.is_empty() {
        return None;
    }
    let mut process = std::process::Command::new("cmd");
    process.arg("/C").arg(command);
    process
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .ok()
}

fn user_cache_home() -> PathBuf {
    if let Some(cache) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(cache);
    }
    if let Some(profile) = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("AppData").join("Local"))
    {
        return profile;
    }
    std::env::temp_dir()
}

pub(crate) fn stable_hash(parts: &[&str]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn entry_url(config: &WebView2Config) -> WebView2Result<String> {
    if let Some(url) = &config.dev_url {
        return Ok(url.clone());
    }
    if let Some(url) = &config.url {
        return Ok(url.clone());
    }
    let Some(entry) = &config.entry else {
        return Err(WebView2Error::Backend(
            "WebView2 window has no entry, URL, or dev URL".to_string(),
        ));
    };
    let (entry_path, suffix) = split_entry_suffix(entry);
    let path = std::path::PathBuf::from(entry_path);
    let canonical = path.canonicalize().unwrap_or(path);
    Ok(format!("{}{}", path_to_file_url(&canonical), suffix))
}

fn split_entry_suffix(entry: &str) -> (&str, &str) {
    let split = [entry.find('?'), entry.find('#')]
        .into_iter()
        .flatten()
        .min();
    match split {
        Some(index) => (&entry[..index], &entry[index..]),
        None => (entry, ""),
    }
}

fn path_to_file_url(path: &std::path::Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/");
    // `canonicalize()` on Windows prefixes paths with `\\?\` / `\\?\UNC\`.
    // Those are not valid in file URLs and WebView2 rejects them with
    // E_INVALIDARG from Navigate.
    if let Some(stripped) = text.strip_prefix("//?/UNC/") {
        return format!("file://{stripped}");
    }
    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_string();
    }
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

#[cfg(test)]
mod entry_url_tests {
    use super::path_to_file_url;
    use std::path::Path;

    #[test]
    fn strips_windows_verbatim_prefix() {
        let url = path_to_file_url(Path::new(r"\\?\C:\Users\test\app\index.html"));
        assert_eq!(url, "file:///C:/Users/test/app/index.html");
    }

    #[test]
    fn strips_windows_verbatim_unc_prefix() {
        let url = path_to_file_url(Path::new(r"\\?\UNC\server\share\index.html"));
        assert_eq!(url, "file://server/share/index.html");
    }
}

fn metrics_label(config: &WebView2Config) -> String {
    config
        .app_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.title)
        .to_string()
}

fn wait_for_dev_server(url: &str, _event_tx: &Sender<WebView2UserEvent>) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if dev_server_reachable(url) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn dev_server_reachable(url: &str) -> bool {
    let Some((_scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let port = authority
        .rsplit(':')
        .next()
        .and_then(|value| value.parse::<u16>().ok());
    let Some(port) = port else {
        return true;
    };
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .rsplit(':')
        .next()
        .unwrap_or(authority);

    let addrs: Vec<std::net::SocketAddr> = if matches!(host, "localhost" | "127.0.0.1" | "::1") {
        vec![
            std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)),
            std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
        ]
    } else {
        match format!("{host}:{port}").to_socket_addrs() {
            Ok(addrs) => addrs.collect(),
            Err(_) => return false,
        }
    };

    addrs.iter().any(|addr| {
        std::net::TcpStream::connect_timeout(addr, Duration::from_millis(100)).is_ok()
    })
}

/// User events that the winit event loop processes in addition to
/// `winit::Event::WindowEvent`. Bridge handlers and the activity
/// emitter send these via an `mpsc::Sender`; the app's
/// `about_to_wait` callback drains the channel and dispatches the
/// events on the UI thread.
#[derive(Debug, Clone)]
pub enum WebView2UserEvent {
    BridgeEvent {
        name: String,
        payload: serde_json::Value,
    },
    Activity {
        update: ActivityHostUpdate,
    },
    /// A guest's popup policy asked for the URL to open in a new guest.
    GuestOpenRequested {
        parent: String,
        url: String,
    },
    /// Deferred `fenestra.guest.*` / popup bridge work. Must not run inside
    /// a WebView2 NavigationStarting callback (controller create deadlocks).
    GuestBridge {
        request_id: String,
        command: fenestra_bridge::BridgeCommand,
    },
    SetVisible(bool),
    Show,
    Hide,
    Focus,
    Minimize,
    Maximize,
    Unmaximize,
    Exit,
}

impl WebView2UserEvent {
    pub(crate) fn dispatch(self, sender: &Sender<Self>) -> bool {
        sender.send(self).is_ok()
    }

    pub(crate) fn dispatch_and_wake(self, inner: &Arc<WebView2ProcessInner>) -> bool {
        let ok = inner.event_sender.send(self).is_ok();
        if let Ok(wake) = inner.wake.lock()
            && let Some(proxy) = wake.as_ref()
        {
            proxy.wake_up();
        }
        ok
    }
}
