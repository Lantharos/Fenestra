use softbuffer::{Context, Surface};
use std::{num::NonZeroU32, sync::Arc};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::{ButtonSource, ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use super::{BG, FILL, MUTED, TEXT, draw_text, fill_rect};

const WIDTH: u32 = 500;
const HEIGHT: u32 = 210;
const BUTTON_HEIGHT: i32 = 38;

pub(crate) fn confirm_update(app_name: &str, version: &str) -> Result<bool, String> {
    let event_loop = EventLoop::new().map_err(|error| error.to_string())?;
    let app = ConfirmApp {
        window: None,
        context: None,
        surface: None,
        cursor: PhysicalPosition::new(0.0, 0.0),
        decision: Decision::default(),
        title: format!("Update {app_name}"),
        message: format!("Version {version} is ready to install."),
    };
    let decision = app.decision.clone();
    event_loop.run_app(app).map_err(|error| error.to_string())?;
    Ok(decision.load())
}

#[derive(Clone, Default)]
struct Decision(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Decision {
    fn set(&self, value: bool) {
        self.0.store(value, std::sync::atomic::Ordering::Release);
    }

    fn load(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

struct ConfirmApp {
    window: Option<Arc<dyn Window>>,
    context: Option<Context<Arc<dyn Window>>>,
    surface: Option<Surface<Arc<dyn Window>, Arc<dyn Window>>>,
    cursor: PhysicalPosition<f64>,
    decision: Decision,
    title: String,
    message: String,
}

impl ApplicationHandler for ConfirmApp {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        self.resumed(event_loop);
    }

    fn resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title(&self.title)
            .with_surface_size(LogicalSize::new(f64::from(WIDTH), f64::from(HEIGHT)))
            .with_resizable(false);
        let window: Arc<dyn Window> = match event_loop.create_window(attributes) {
            Ok(window) => Arc::from(window),
            Err(_) => {
                event_loop.exit();
                return;
            }
        };
        let Ok(context) = Context::new(window.clone()) else {
            event_loop.exit();
            return;
        };
        let Ok(surface) = Surface::new(&context, window.clone()) else {
            event_loop.exit();
            return;
        };
        self.context = Some(context);
        self.surface = Some(surface);
        self.window = Some(window);
        self.paint();
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::PointerMoved { position, .. } => self.cursor = position,
            WindowEvent::PointerButton {
                state: ElementState::Released,
                button: ButtonSource::Mouse(MouseButton::Left),
                position,
                ..
            } => {
                self.cursor = position;
                let Some(window) = &self.window else {
                    return;
                };
                let size = window.surface_size();
                let install = button_rect(size.width, true);
                let later = button_rect(size.width, false);
                if contains(install, self.cursor) {
                    self.decision.set(true);
                    event_loop.exit();
                } else if contains(later, self.cursor) {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested | WindowEvent::SurfaceResized(_) => self.paint(),
            _ => {}
        }
    }
}

impl ConfirmApp {
    fn paint(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, self.surface.as_mut()) else {
            return;
        };
        let size = window.surface_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let (Ok(width_nz), Ok(height_nz)) =
            (NonZeroU32::try_from(width), NonZeroU32::try_from(height))
        else {
            return;
        };
        if surface.resize(width_nz, height_nz).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        buffer.fill(BG);
        draw_text(
            &mut buffer,
            (width, height),
            (24, 32),
            &self.title,
            TEXT,
            2.0,
        );
        draw_text(
            &mut buffer,
            (width, height),
            (24, 72),
            &self.message,
            MUTED,
            1.0,
        );
        let later = button_rect(width, false);
        let install = button_rect(width, true);
        fill_rect(&mut buffer, (width, height), later, 0xFF_2A_2A_2E);
        fill_rect(&mut buffer, (width, height), install, FILL);
        draw_text(
            &mut buffer,
            (width, height),
            (later.0 + 24, later.1 + 15),
            "Later",
            TEXT,
            1.0,
        );
        draw_text(
            &mut buffer,
            (width, height),
            (install.0 + 20, install.1 + 15),
            "Install",
            0xFF_16_16_18,
            1.0,
        );
        let _ = buffer.present();
    }
}

fn button_rect(width: u32, primary: bool) -> (i32, i32, i32, i32) {
    let x = width as i32 - if primary { 132 } else { 248 };
    (x, HEIGHT as i32 - 62, 100, BUTTON_HEIGHT)
}

fn contains(rect: (i32, i32, i32, i32), point: PhysicalPosition<f64>) -> bool {
    let (x, y, width, height) = rect;
    point.x >= f64::from(x)
        && point.x < f64::from(x + width)
        && point.y >= f64::from(y)
        && point.y < f64::from(y + height)
}
