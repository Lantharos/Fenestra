use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use winit::{
    data_transfer::{DataTransferSendBuilder, SendData, TypeHint},
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta},
    event_loop::{ActiveEventLoop, DndAction},
    keyboard::Key,
};

use crate::osr::host::native::OsrNativeHost;
use crate::osr::host::types::{
    ClickMemory, EVENTFLAG_ALT_DOWN, EVENTFLAG_COMMAND_DOWN, EVENTFLAG_CONTROL_DOWN,
    EVENTFLAG_IS_REPEAT, EVENTFLAG_LEFT_MOUSE_BUTTON, EVENTFLAG_MIDDLE_MOUSE_BUTTON,
    EVENTFLAG_PRECISION_SCROLLING_DELTA, EVENTFLAG_RIGHT_MOUSE_BUTTON, EVENTFLAG_SHIFT_DOWN,
};
use crate::osr::protocol::{FileDragRequest, encode_component};

impl OsrNativeHost {
    pub(in crate::osr::host) fn start_file_drag(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        request: FileDragRequest,
    ) {
        let FileDragRequest { paths, x: _, y: _ } = request;
        let Some(window) = self.window.clone() else {
            self.finish_file_drag(None);
            return;
        };

        let paths: Vec<PathBuf> = paths
            .into_iter()
            .map(|path| PathBuf::from(path.trim()))
            .filter(|path| !path.as_os_str().is_empty())
            .collect();
        if paths.is_empty() {
            self.finish_file_drag(None);
            return;
        }

        let transfer = DataTransferSendBuilder::new(paths)
            .with_type(TypeHint::UriList, |paths, _| {
                SendData::from_file_paths(paths.iter())
            })
            .build();

        match event_loop.start_drag(
            window.id(),
            transfer,
            &[DndAction::Copy, DndAction::Move],
            None,
        ) {
            Ok(id) => {
                self.active_file_drag = Some(id);
            }
            Err(error) => {
                eprintln!("failed to start native file drag: {error}");
                self.finish_file_drag(None);
            }
        }
    }

    pub(in crate::osr::host) fn finish_file_drag(&mut self, action: Option<DndAction>) {
        self.active_file_drag = None;
        let operation = match action {
            Some(DndAction::Copy) => "copy",
            Some(DndAction::Move) => "move",
            Some(DndAction::Link) => "link",
            _ => "none",
        };
        let (x, y) = self
            .content_position(self.cursor_x, self.cursor_y)
            .unwrap_or((self.cursor_x, self.cursor_y));
        self.send_control(&format!(
            "file_drag_ended\t{:.0}\t{:.0}\t{operation}\n",
            x, y
        ));
    }

    pub(in crate::osr::host) fn forward_mouse_move(&self, leave: bool) {
        if let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) {
            self.send_control(&format!(
                "mouse_move\t{:.2}\t{:.2}\t{}\t{}\n",
                x,
                y,
                self.input_modifiers(),
                i32::from(leave)
            ));
        }
    }

    pub(in crate::osr::host) fn forward_mouse_click(
        &self,
        button: Option<MouseButton>,
        up: bool,
        click_count: i32,
    ) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let Some(button) = cef_mouse_button(button) else {
            return;
        };
        self.send_control(&format!(
            "mouse_click\t{:.2}\t{:.2}\t{}\t{}\t{}\t{}\n",
            x,
            y,
            button,
            self.input_modifiers(),
            i32::from(up),
            click_count.max(1)
        ));
    }

    pub(in crate::osr::host) fn forward_navigation_button(&self, button: Option<MouseButton>) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let button = match button {
            Some(MouseButton::Back) => 3,
            Some(MouseButton::Forward) => 4,
            _ => return,
        };
        self.send_control(&format!(
            "mouse_navigation\t{:.2}\t{:.2}\t{}\t{}\n",
            x,
            y,
            button,
            self.input_modifiers()
        ));
    }

    pub(in crate::osr::host) fn forward_mouse_wheel(&self, delta: MouseScrollDelta) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let (dx, dy, precision) = match delta {
            MouseScrollDelta::LineDelta(x, y) => ((x * 120.0) as i32, (y * 120.0) as i32, false),
            MouseScrollDelta::PixelDelta(position) => (position.x as i32, position.y as i32, true),
            _ => return,
        };
        self.send_control(&format!(
            "mouse_wheel\t{:.2}\t{:.2}\t{}\t{}\t{}\n",
            x,
            y,
            dx,
            dy,
            self.input_modifiers()
                | if precision {
                    EVENTFLAG_PRECISION_SCROLLING_DELTA
                } else {
                    0
                }
        ));
    }

    pub(in crate::osr::host) fn send_key_event(&self, event: &KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        let text = if pressed {
            event
                .text
                .as_deref()
                .filter(|text| should_send_char_text(text))
                .unwrap_or("")
        } else {
            ""
        };
        self.send_control(&format!(
            "key\t{}\t{}\t{}\t{}\t{}\n",
            i32::from(pressed),
            encode_component(&key_name(event)),
            encode_component(text),
            self.input_modifiers() | if event.repeat { EVENTFLAG_IS_REPEAT } else { 0 },
            i32::from(event.repeat)
        ));
    }

    pub(in crate::osr::host) fn input_modifiers(&self) -> u32 {
        let mut modifiers = 0;
        if self.modifiers.shift_key() {
            modifiers |= EVENTFLAG_SHIFT_DOWN;
        }
        if self.modifiers.control_key() {
            modifiers |= EVENTFLAG_CONTROL_DOWN;
        }
        if self.modifiers.alt_key() {
            modifiers |= EVENTFLAG_ALT_DOWN;
        }
        if self.modifiers.meta_key() {
            modifiers |= EVENTFLAG_COMMAND_DOWN;
        }
        if self.mouse.left {
            modifiers |= EVENTFLAG_LEFT_MOUSE_BUTTON;
        }
        if self.mouse.middle {
            modifiers |= EVENTFLAG_MIDDLE_MOUSE_BUTTON;
        }
        if self.mouse.right {
            modifiers |= EVENTFLAG_RIGHT_MOUSE_BUTTON;
        }
        modifiers
    }

    pub(in crate::osr::host) fn set_mouse_button(
        &mut self,
        button: Option<MouseButton>,
        pressed: bool,
    ) {
        match button {
            Some(MouseButton::Left) => self.mouse.left = pressed,
            Some(MouseButton::Middle) => self.mouse.middle = pressed,
            Some(MouseButton::Right) => self.mouse.right = pressed,
            _ => {}
        }
    }

    pub(in crate::osr::host) fn next_click_count(&mut self, button: Option<MouseButton>) -> i32 {
        let Some(button) = button else {
            return 1;
        };
        let now = Instant::now();
        let count = self
            .last_click
            .filter(|last| {
                last.button == button
                    && now.duration_since(last.at) <= Duration::from_millis(500)
                    && (last.x - self.cursor_x).abs() <= 4.0
                    && (last.y - self.cursor_y).abs() <= 4.0
            })
            .map(|last| (last.count + 1).min(3))
            .unwrap_or(1);
        self.last_click = Some(ClickMemory {
            button,
            x: self.cursor_x,
            y: self.cursor_y,
            at: now,
            count,
        });
        count
    }
}

fn cef_mouse_button(button: Option<MouseButton>) -> Option<&'static str> {
    match button {
        Some(MouseButton::Left) => Some("left"),
        Some(MouseButton::Middle) => Some("middle"),
        Some(MouseButton::Right) => Some("right"),
        _ => None,
    }
}

fn key_name(event: &KeyEvent) -> String {
    match event.logical_key.as_ref() {
        Key::Character(value) if !value.is_empty() => value.to_string(),
        Key::Named(named) => named.to_string(),
        _ => match &event.physical_key {
            winit::keyboard::PhysicalKey::Code(code) => format!("{code:?}"),
            _ => "Unidentified".to_string(),
        },
    }
}

fn should_send_char_text(text: &str) -> bool {
    !text.chars().any(char::is_control)
}
