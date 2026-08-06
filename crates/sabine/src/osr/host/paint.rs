use std::time::Instant;

use sabine_platform::request_window_effect;
use winit::event_loop::ControlFlow;

use crate::osr::protocol::{
    MAIN_TEXTURE_ID, OsrFrame, OsrPaintBatch, OsrSurface, POPUP_OVERLAY_ID,
};
use crate::render::{DisplayList, ImageCommand, RectCommand, RoundedRectCommand};
use crate::window::style::Color;

use super::native::OsrNativeHost;
use super::paint_upload::upload_composed_damage;
use super::types::{
    LifecycleState, PendingResizePaint, RESIZE_REPAINT_GRACE, RESIZE_REPAINT_RETRY,
    overlay_id_for_surface, overlay_texture_id, uses_sabine_chrome,
};

impl OsrNativeHost {
    pub(super) fn send_resize(&self) {
        let (width, height, scale) = self.content_size_for_cef();
        self.send_control(&format!("resize\t{width}\t{height}\t{scale:.4}\n"));
    }

    pub(super) fn queue_resize_paint(&mut self) {
        let size = self.content_surface_size();
        if self.main_frame_matches(size) {
            // Content size unchanged — do not poke CEF (WasResized/Invalidate
            // blanks OSR until the next paint, which shows as a drag-end flash).
            self.pending_resize_paint = None;
            return;
        }
        let now = Instant::now();
        self.pending_resize_paint = Some(PendingResizePaint {
            size,
            retry_at: now + RESIZE_REPAINT_RETRY,
            deadline: now + RESIZE_REPAINT_GRACE,
        });
        self.send_resize();
    }

    pub(super) fn retry_resize_paint(&mut self) {
        let Some(mut pending) = self.pending_resize_paint else {
            return;
        };
        if self.main_frame_matches(pending.size) {
            self.pending_resize_paint = None;
            return;
        }
        self.send_resize();
        pending.retry_at = Instant::now() + RESIZE_REPAINT_RETRY;
        self.pending_resize_paint = Some(pending);
    }

    pub(super) fn clear_pending_resize_paint(&mut self) {
        let Some(pending) = self.pending_resize_paint else {
            return;
        };
        if self.main_frame_matches(pending.size) {
            self.pending_resize_paint = None;
        }
    }

    pub(super) fn should_accept_main_frame_size(
        &self,
        size: (u32, u32),
        target: (u32, u32),
    ) -> bool {
        if size == target {
            return true;
        }
        if self.pending_resize_paint.is_none() {
            return false;
        }
        let current_distance = self
            .main_frame_size()
            .map_or(u64::MAX, |current| size_distance(current, target));
        size_distance(size, target) < current_distance
    }

    pub(super) fn main_frame_size(&self) -> Option<(u32, u32)> {
        self.main_frame
            .as_ref()
            .map(|frame| (frame.width, frame.height))
    }

    pub(super) fn main_frame_matches(&self, size: (u32, u32)) -> bool {
        self.main_frame_size().is_some_and(|frame| frame == size)
    }

    pub(super) fn accepts_paint(&self) -> bool {
        // Keep compositing while FPS-throttled (blur/occlusion suspend). Only
        // stop accepting paints when the view is actually gone.
        self.config.visible
            && !matches!(
                self.lifecycle_state,
                LifecycleState::Hibernating | LifecycleState::Hibernated
            )
    }

    pub(super) fn update_frame_texture(&mut self, frame: OsrFrame) -> bool {
        if self.renderer.is_none() {
            return false;
        }
        let (width, height, _) = self.content_size_for_cef();
        match frame.surface {
            OsrSurface::Main => {
                let Some(damage) = self.main_buffer.compose(width, height, &frame) else {
                    return false;
                };
                let Some(renderer) = self.renderer.as_mut() else {
                    return false;
                };
                if upload_composed_damage(
                    renderer,
                    MAIN_TEXTURE_ID,
                    width,
                    height,
                    self.main_buffer.bytes(),
                    damage,
                    std::slice::from_ref(&frame),
                )
                .is_err()
                {
                    return false;
                }
                self.main_frame = Some(OsrFrame {
                    surface: OsrSurface::Main,
                    width,
                    height,
                    x: 0,
                    y: 0,
                    bytes: Vec::new(),
                });
                self.clear_pending_resize_paint();
            }
            OsrSurface::Popup | OsrSurface::Guest(_) => {
                let Some(overlay_id) = overlay_id_for_surface(&frame.surface) else {
                    return false;
                };
                let texture_id = overlay_texture_id(&overlay_id);
                let local = overlay_local_frame(&frame);
                let damage = {
                    let overlay = self.overlays.entry(overlay_id.clone()).or_insert_with(|| {
                        super::types::OverlayLayer {
                            frame: local.clone(),
                            buffer: crate::osr::frame_buffer::FrameBuffer::new(),
                        }
                    });
                    let Some(damage) = overlay.buffer.compose(frame.width, frame.height, &local)
                    else {
                        return false;
                    };
                    damage
                };
                let bytes = self
                    .overlays
                    .get(&overlay_id)
                    .map(|overlay| overlay.buffer.bytes().to_vec())
                    .unwrap_or_default();
                let Some(renderer) = self.renderer.as_mut() else {
                    return false;
                };
                if upload_composed_damage(
                    renderer,
                    &texture_id,
                    frame.width,
                    frame.height,
                    &bytes,
                    damage,
                    std::slice::from_ref(&local),
                )
                .is_err()
                {
                    return false;
                }
                if let Some(overlay) = self.overlays.get_mut(&overlay_id) {
                    overlay.frame = OsrFrame {
                        surface: frame.surface.clone(),
                        width: frame.width,
                        height: frame.height,
                        x: frame.x,
                        y: frame.y,
                        bytes: Vec::new(),
                    };
                }
            }
        }
        true
    }

    pub(super) fn clear_overlay(&mut self, overlay_id: &str) {
        self.overlays.remove(overlay_id);
    }

    pub(super) fn update_paint_batch(&mut self, batch: OsrPaintBatch) -> bool {
        let content_size = self.content_surface_size();
        let batch_size = (batch.width, batch.height);
        if batch.frames.is_empty() {
            return false;
        }
        match batch.surface {
            OsrSurface::Main => {
                if !self.should_accept_main_frame_size(batch_size, content_size) {
                    self.retry_resize_paint();
                    return false;
                }
                let Some(renderer) = self.renderer.as_mut() else {
                    return false;
                };
                let Some(damage) =
                    self.main_buffer
                        .compose_batch(batch.width, batch.height, &batch.frames)
                else {
                    return false;
                };
                if upload_composed_damage(
                    renderer,
                    MAIN_TEXTURE_ID,
                    batch.width,
                    batch.height,
                    self.main_buffer.bytes(),
                    damage,
                    &batch.frames,
                )
                .is_err()
                {
                    return false;
                }
                self.main_frame = Some(OsrFrame {
                    surface: OsrSurface::Main,
                    width: batch.width,
                    height: batch.height,
                    x: 0,
                    y: 0,
                    bytes: Vec::new(),
                });
                if batch_size == content_size {
                    self.clear_pending_resize_paint();
                }
            }
            OsrSurface::Popup | OsrSurface::Guest(_) => {
                let Some(overlay_id) = overlay_id_for_surface(&batch.surface) else {
                    return false;
                };
                let texture_id = overlay_texture_id(&overlay_id);
                let (damage, bytes) = {
                    let overlay = self.overlays.entry(overlay_id.clone()).or_insert_with(|| {
                        super::types::OverlayLayer {
                            frame: OsrFrame {
                                surface: batch.surface.clone(),
                                width: batch.width,
                                height: batch.height,
                                x: batch.x,
                                y: batch.y,
                                bytes: Vec::new(),
                            },
                            buffer: crate::osr::frame_buffer::FrameBuffer::new(),
                        }
                    });
                    let Some(damage) =
                        overlay
                            .buffer
                            .compose_batch(batch.width, batch.height, &batch.frames)
                    else {
                        return false;
                    };
                    (damage, overlay.buffer.bytes().to_vec())
                };
                let Some(renderer) = self.renderer.as_mut() else {
                    return false;
                };
                if upload_composed_damage(
                    renderer,
                    &texture_id,
                    batch.width,
                    batch.height,
                    &bytes,
                    damage,
                    &batch.frames,
                )
                .is_err()
                {
                    return false;
                }
                if let Some(overlay) = self.overlays.get_mut(&overlay_id) {
                    overlay.frame = OsrFrame {
                        surface: batch.surface.clone(),
                        width: batch.width,
                        height: batch.height,
                        x: batch.x,
                        y: batch.y,
                        bytes: Vec::new(),
                    };
                }
            }
        }
        true
    }

    pub(super) fn present_after_first_frame(&mut self) {
        if self.presented {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        self.presented = true;
        super::trace_host(&self.config, "first_paint");
        // Drop any prior effect before binding a new one; Wayland allows only one
        // `ext_background_effect` resource per surface.
        self.effect = None;
        self.effect = request_window_effect(&window, &self.window_options());
        self.update_effect_regions();
        if self.config.visible {
            window.set_visible(true);
            window.set_minimized(false);
            if self.config.active || self.focused {
                super::native::present_window(&window);
            }
        }
        if self.config.visible {
            window.request_redraw();
        }
    }

    pub(super) fn render(&mut self) {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor()) as f32;
        let width = self.surface_size.width as f32 / scale.max(1.0);
        let height = self.surface_size.height as f32 / scale.max(1.0);
        let list = self.display_list(width.max(1.0), height.max(1.0));
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Err(error) = renderer.render(&list) {
            eprintln!("Sabine OSR render failed: {error}");
            return;
        }
        if self.effect_regions_dirty {
            self.effect_regions_dirty = false;
            self.update_effect_regions();
        }
    }

    pub(super) fn display_list(&self, width: f32, height: f32) -> DisplayList {
        let ready = self.main_frame.is_some();
        let background = if !ready || self.config.transparent {
            Color::rgba(0.0, 0.0, 0.0, 0.0)
        } else {
            Color::WINDOW
        };
        let mut list = DisplayList::new(background);
        if !ready {
            return list;
        }
        if !self.config.transparent || uses_sabine_chrome(self.config.chrome) {
            let radius = if self.config.chrome.uses_native_decorations() {
                0.0
            } else {
                12.0
            };
            list.push(RoundedRectCommand {
                x: 0.0,
                y: 0.0,
                width,
                height,
                radius,
                color: Color::rgba(
                    0.08,
                    0.08,
                    0.08,
                    if self.config.transparent { 0.38 } else { 1.0 },
                ),
            });
        }
        // Solid underlay for opaque regions so glass windows only show blur
        // through the sidebar (or other non-opaque areas), not the content pane.
        if self.config.transparent
            && let Some(opaque) = &self.config.regions.opaque
        {
            let region_width = width.round().max(1.0) as i32;
            let region_height = height.round().max(1.0) as i32;
            for rect in opaque.resolved_rects(region_width, region_height) {
                list.push(RectCommand {
                    x: rect.x as f32,
                    y: rect.y as f32,
                    width: rect.width as f32,
                    height: rect.height as f32,
                    color: Color::WINDOW,
                });
            }
        }
        self.draw_titlebar(&mut list, width);
        let y = self.titlebar_height();
        if let Some(frame) = &self.main_frame {
            list.push(ImageCommand {
                id: MAIN_TEXTURE_ID.to_string(),
                x: 0.0,
                y,
                width: frame.width as f32,
                height: frame.height as f32,
                opacity: 1.0,
            });
        }
        for (overlay_id, overlay) in &self.overlays {
            if overlay_id.as_str() == POPUP_OVERLAY_ID {
                continue;
            }
            list.push(ImageCommand {
                id: overlay_texture_id(overlay_id),
                x: overlay.frame.x as f32,
                y: y + overlay.frame.y as f32,
                width: overlay.frame.width as f32,
                height: overlay.frame.height as f32,
                opacity: 1.0,
            });
        }
        if let Some(overlay) = self.overlays.get(POPUP_OVERLAY_ID) {
            list.push(ImageCommand {
                id: overlay_texture_id(POPUP_OVERLAY_ID),
                x: overlay.frame.x as f32,
                y: y + overlay.frame.y as f32,
                width: overlay.frame.width as f32,
                height: overlay.frame.height as f32,
                opacity: 1.0,
            });
        }
        list
    }
}

impl OsrNativeHost {
    pub(super) fn drive_resize_paint(
        &mut self,
        event_loop: &dyn winit::event_loop::ActiveEventLoop,
    ) -> bool {
        let Some(pending) = self.pending_resize_paint else {
            return false;
        };
        if !self.config.visible
            || self.lifecycle_state != LifecycleState::Active
            || self.window.is_none()
        {
            self.pending_resize_paint = None;
            return false;
        }
        if self.main_frame_matches(pending.size) {
            self.pending_resize_paint = None;
            return false;
        }
        let now = Instant::now();
        if now >= pending.deadline {
            self.pending_resize_paint = None;
            return false;
        }
        if now >= pending.retry_at {
            self.retry_resize_paint();
        }
        if let Some(pending) = self.pending_resize_paint {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                pending.retry_at.min(pending.deadline),
            ));
            return true;
        }
        false
    }
}

fn overlay_local_frame(frame: &OsrFrame) -> OsrFrame {
    OsrFrame {
        surface: frame.surface.clone(),
        width: frame.width,
        height: frame.height,
        x: 0,
        y: 0,
        bytes: frame.bytes.clone(),
    }
}

fn size_distance(size: (u32, u32), target: (u32, u32)) -> u64 {
    u64::from(size.0.abs_diff(target.0)) + u64::from(size.1.abs_diff(target.1))
}
