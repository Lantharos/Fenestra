use std::sync::Arc;

use mullion_platform::WindowRegionRect;
use winit::{
    cursor::{Cursor, CursorIcon},
    event_loop::ActiveEventLoop,
    window::{ResizeDirection, Window as WinitWindow},
};

use crate::MullionWindowControlRegion;
use crate::render::{DisplayList, RectCommand, RoundedRectCommand, TextCommand};
use crate::window::style::Color;

use super::native::OsrNativeHost;
use super::types::{CONTROL_GAP, CONTROL_SIZE, ControlRect, RESIZE_EDGE, TitlebarControl};

impl OsrNativeHost {
    pub(super) fn draw_titlebar(&self, list: &mut DisplayList, width: f32) {
        let titlebar_height = self.titlebar_height();
        if titlebar_height == 0.0 {
            return;
        }
        let titlebar_color = Color::rgba(0.07, 0.07, 0.075, 0.58);
        let titlebar_radius = 12.0;
        list.push(RoundedRectCommand {
            x: 0.0,
            y: 0.0,
            width,
            height: titlebar_height,
            radius: titlebar_radius,
            color: titlebar_color,
        });
        list.push(RectCommand {
            x: 0.0,
            y: (titlebar_height - titlebar_radius).max(0.0),
            width,
            height: titlebar_radius.min(titlebar_height),
            color: titlebar_color,
        });
        list.push(RectCommand {
            x: 0.0,
            y: titlebar_height - 1.0,
            width,
            height: 1.0,
            color: Color::WHITE.opacity(0.10),
        });
        list.push(TextCommand {
            text: self.config.title.clone(),
            x: 0.0,
            y: 8.0,
            width,
            height: 22.0,
            size: 14.0,
            line_height: 20.0,
            color: Color::TEXT,
        });
        for control in [
            TitlebarControl::Minimize,
            TitlebarControl::Maximize,
            TitlebarControl::Close,
        ] {
            draw_control(
                list,
                control_rect(width, titlebar_height, control),
                control,
                self.hovered_control == Some(control),
                self.pressed_control == Some(control),
            );
        }
    }

    pub(super) fn update_titlebar_hover(&mut self) {
        let width = self.logical_width();
        let next = self.control_at(width, self.cursor_x, self.cursor_y);
        self.hovered_control = next;
    }

    pub(super) fn set_cursor(&mut self, cursor: CursorIcon) {
        if self.cursor == cursor {
            return;
        }
        self.cursor = cursor;
        if let Some(window) = &self.window {
            window.set_cursor(Cursor::Icon(cursor));
        }
    }

    pub(super) fn set_native_cursor(&mut self, cursor: CursorIcon) {
        self.native_cursor_override = true;
        self.set_cursor(cursor);
    }

    pub(super) fn set_content_cursor(&mut self, cursor: CursorIcon) {
        self.native_cursor_override = false;
        self.set_cursor(cursor);
    }

    pub(super) fn clear_native_cursor(&mut self) {
        if !self.native_cursor_override {
            return;
        }
        self.native_cursor_override = false;
        self.set_cursor(CursorIcon::Default);
    }

    pub(super) fn control_at(&self, width: f32, x: f32, y: f32) -> Option<TitlebarControl> {
        if let Some(control) = configured_control_at(&self.config.control_regions, width, x, y) {
            return Some(control);
        }
        titlebar_control_at(width, self.titlebar_height(), x, y)
    }

    pub(super) fn is_drag_region(&self, width: f32, x: f32, y: f32) -> bool {
        let page_y = y - self.titlebar_height();
        let page_excluded = page_y >= 0.0
            && configured_region_at(&self.page_drag_exclusion_regions, width, x, page_y);
        if configured_region_at(&self.config.drag_exclusion_regions, width, x, y) || page_excluded {
            return false;
        }
        if !self.config.drag_regions.is_empty() || !self.page_drag_regions.is_empty() {
            let page_draggable =
                page_y >= 0.0 && configured_region_at(&self.page_drag_regions, width, x, page_y);
            return configured_region_at(&self.config.drag_regions, width, x, y) || page_draggable;
        }
        self.titlebar_height() > 0.0 && y <= self.titlebar_height()
    }
}

pub(super) fn configured_control_at(
    controls: &[MullionWindowControlRegion],
    width: f32,
    x: f32,
    y: f32,
) -> Option<TitlebarControl> {
    controls.iter().find_map(|region| {
        rect_region_contains(&region.rect, width, x, y).then(|| match region.action {
            crate::MullionWindowControlAction::Minimize => TitlebarControl::Minimize,
            crate::MullionWindowControlAction::Maximize => TitlebarControl::Maximize,
            crate::MullionWindowControlAction::Close => TitlebarControl::Close,
        })
    })
}

pub(super) fn configured_region_at(
    regions: &[WindowRegionRect],
    width: f32,
    x: f32,
    y: f32,
) -> bool {
    regions
        .iter()
        .any(|region| rect_region_contains(region, width, x, y))
}

fn rect_region_contains(region: &WindowRegionRect, width: f32, x: f32, y: f32) -> bool {
    let region_x = if region.x < 0 {
        width + region.x as f32
    } else {
        region.x as f32
    };
    let region_width = if region.width == i32::MAX {
        width - region_x
    } else {
        region.width as f32
    };
    let rect = ControlRect::new(
        region_x,
        region.y as f32,
        region_width.max(0.0),
        region.height as f32,
    );
    rect_contains(rect, x, y)
}

pub(super) fn control_rect(
    width: f32,
    titlebar_height: f32,
    control: TitlebarControl,
) -> ControlRect {
    let right = width - 12.0;
    let y = (titlebar_height - CONTROL_SIZE) * 0.5;
    let index = match control {
        TitlebarControl::Close => 0.0,
        TitlebarControl::Maximize => 1.0,
        TitlebarControl::Minimize => 2.0,
    };
    ControlRect::new(
        right - CONTROL_SIZE * (index + 1.0) - CONTROL_GAP * index,
        y,
        CONTROL_SIZE,
        CONTROL_SIZE,
    )
}

pub(super) fn titlebar_control_at(
    width: f32,
    titlebar_height: f32,
    x: f32,
    y: f32,
) -> Option<TitlebarControl> {
    if titlebar_height == 0.0 || y < 0.0 || y > titlebar_height {
        return None;
    }
    [
        TitlebarControl::Minimize,
        TitlebarControl::Maximize,
        TitlebarControl::Close,
    ]
    .into_iter()
    .find(|control| rect_contains(control_rect(width, titlebar_height, *control), x, y))
}

fn draw_control(
    list: &mut DisplayList,
    rect: ControlRect,
    control: TitlebarControl,
    hovered: bool,
    pressed: bool,
) {
    let fill_alpha = if pressed {
        0.24
    } else if hovered {
        0.15
    } else {
        0.10
    };
    list.push(RoundedRectCommand {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        radius: 999.0,
        color: Color::TEXT.opacity(fill_alpha),
    });
    let icon = Color::TEXT.opacity(if hovered || pressed { 0.95 } else { 0.68 });
    match control {
        TitlebarControl::Minimize => list.push(RectCommand {
            x: rect.x + (rect.width - 9.0) * 0.5,
            y: rect.y + rect.height * 0.5 - 0.75,
            width: 9.0,
            height: 1.5,
            color: icon,
        }),
        TitlebarControl::Maximize => draw_maximize(list, rect, icon),
        TitlebarControl::Close => draw_close(list, rect, icon),
    }
}

fn draw_maximize(list: &mut DisplayList, rect: ControlRect, color: Color) {
    let x = rect.x + (rect.width - 9.0) * 0.5;
    let y = rect.y + (rect.height - 9.0) * 0.5;
    for command in [
        RectCommand {
            x,
            y,
            width: 9.0,
            height: 1.5,
            color,
        },
        RectCommand {
            x,
            y: y + 7.5,
            width: 9.0,
            height: 1.5,
            color,
        },
        RectCommand {
            x,
            y,
            width: 1.5,
            height: 9.0,
            color,
        },
        RectCommand {
            x: x + 7.5,
            y,
            width: 1.5,
            height: 9.0,
            color,
        },
    ] {
        list.push(command);
    }
}

fn draw_close(list: &mut DisplayList, rect: ControlRect, color: Color) {
    let center_x = rect.x + rect.width * 0.5;
    let center_y = rect.y + rect.height * 0.5;
    for (dx, dy) in [
        (-4.0, -4.0),
        (-2.0, -2.0),
        (0.0, 0.0),
        (2.0, 2.0),
        (4.0, 4.0),
        (-4.0, 4.0),
        (-2.0, 2.0),
        (2.0, -2.0),
        (4.0, -4.0),
    ] {
        list.push(RectCommand {
            x: center_x + dx - 0.9,
            y: center_y + dy - 0.9,
            width: 1.8,
            height: 1.8,
            color,
        });
    }
}

fn rect_contains(rect: ControlRect, x: f32, y: f32) -> bool {
    x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height
}

pub(super) fn resize_direction_at(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> Option<ResizeDirection> {
    if width <= RESIZE_EDGE * 2.0 || height <= RESIZE_EDGE * 2.0 {
        return None;
    }

    let left = x <= RESIZE_EDGE;
    let right = x >= width - RESIZE_EDGE;
    let top = y <= RESIZE_EDGE;
    let bottom = y >= height - RESIZE_EDGE;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (_, true, true, _) => Some(ResizeDirection::NorthEast),
        (true, _, _, true) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::West),
        (_, true, _, _) => Some(ResizeDirection::East),
        (_, _, true, _) => Some(ResizeDirection::North),
        (_, _, _, true) => Some(ResizeDirection::South),
        _ => None,
    }
}

pub(super) fn activate_control(
    host: &mut OsrNativeHost,
    event_loop: &dyn ActiveEventLoop,
    window: &Arc<dyn WinitWindow>,
    control: TitlebarControl,
) {
    match control {
        TitlebarControl::Minimize => {
            if host.config.lifecycle.suspend_on_minimize {
                host.suspend("minimize");
            }
            window.set_minimized(true);
        }
        TitlebarControl::Maximize => window.set_maximized(!window.is_maximized()),
        TitlebarControl::Close => host.begin_close(event_loop),
    }
}
