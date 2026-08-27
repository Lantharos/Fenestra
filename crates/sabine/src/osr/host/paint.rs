use std::time::Instant;

use sabine_platform::request_window_effect;

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
        let now = Instant::now();
        if now < pending.retry_at {
            return;
        }
        self.send_resize();
        pending.retry_at = now + RESIZE_REPAINT_RETRY;
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
        size == target
    }

    pub(super) fn main_frame_size(&self) -> Option<(u32, u32)> {
        self.main_frame
            .as_ref()
            .map(|frame| (frame.width, frame.height))
    }

    pub(super) fn main_frame_matches(&self, size: (u32, u32)) -> bool {
        self.main_frame_size().is_some_and(|frame| frame == size)
    }

    pub(super) fn main_surface_ready(&self) -> bool {
        self.main_load_ready && self.main_frame.is_some()
    }

    pub(super) fn frame_size_for_view(&self, size: (u32, u32)) -> (u32, u32) {
        let scale = self
            .window
            .as_ref()
            .map_or(self.scale_factor, |window| window.scale_factor())
            .max(1.0);
        (
            (f64::from(size.0) / scale).round().max(1.0) as u32,
            (f64::from(size.1) / scale).round().max(1.0) as u32,
        )
    }

    pub(super) fn accepts_paint(&self) -> bool {
        // Keep compositing while FPS-throttled (blur/occlusion suspend). Only
        // stop accepting paints when the view is actually gone.
        (self.config.visible || self.config.lifecycle.retain_hidden_frame)
            && !matches!(
                self.lifecycle_state,
                LifecycleState::Hibernating | LifecycleState::Hibernated
            )
    }

    pub(super) fn update_frame_texture(&mut self, frame: OsrFrame) -> bool {
        if self.renderer.is_none() {
            return false;
        }
        match frame.surface {
            OsrSurface::Main => {
                let target = self.content_surface_size();
                let frame_size = self.frame_size_for_view((frame.width, frame.height));
                if !self.should_accept_main_frame_size(frame_size, target) {
                    self.retry_resize_paint();
                    return false;
                }
                let Some(damage) = self.main_buffer.compose(frame.width, frame.height, &frame)
                else {
                    return false;
                };
                let Some(renderer) = self.renderer.as_mut() else {
                    return false;
                };
                if upload_composed_damage(
                    renderer,
                    MAIN_TEXTURE_ID,
                    frame.width,
                    frame.height,
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
                    width: frame_size.0,
                    height: frame_size.1,
                    x: 0,
                    y: 0,
                    bytes: Vec::new().into(),
                });
                if self.main_load_ready {
                    self.loading = None;
                }
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
                let Some(renderer) = self.renderer.as_mut() else {
                    return false;
                };
                let Some(overlay) = self.overlays.get(&overlay_id) else {
                    return false;
                };
                if upload_composed_damage(
                    renderer,
                    &texture_id,
                    frame.width,
                    frame.height,
                    overlay.buffer.bytes(),
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
                        bytes: Vec::new().into(),
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
                let view_size = self.frame_size_for_view(batch_size);
                if !self.should_accept_main_frame_size(view_size, content_size) {
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
                    width: view_size.0,
                    height: view_size.1,
                    x: 0,
                    y: 0,
                    bytes: Vec::new().into(),
                });
                if self.main_load_ready {
                    self.loading = None;
                }
                if view_size == content_size {
                    self.clear_pending_resize_paint();
                }
            }
            OsrSurface::Popup | OsrSurface::Guest(_) => {
                let Some(overlay_id) = overlay_id_for_surface(&batch.surface) else {
                    return false;
                };
                let texture_id = overlay_texture_id(&overlay_id);
                let damage = {
                    let overlay = self.overlays.entry(overlay_id.clone()).or_insert_with(|| {
                        super::types::OverlayLayer {
                            frame: OsrFrame {
                                surface: batch.surface.clone(),
                                width: batch.width,
                                height: batch.height,
                                x: batch.x,
                                y: batch.y,
                                bytes: Vec::new().into(),
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
                    damage
                };
                let Some(renderer) = self.renderer.as_mut() else {
                    return false;
                };
                let Some(overlay) = self.overlays.get(&overlay_id) else {
                    return false;
                };
                if upload_composed_damage(
                    renderer,
                    &texture_id,
                    batch.width,
                    batch.height,
                    overlay.buffer.bytes(),
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
                        bytes: Vec::new().into(),
                    };
                }
            }
        }
        true
    }

    pub(super) fn present_rendered_surface(&mut self, trace: &str) {
        if self.presented {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        self.presented = true;
        super::trace_host(&self.config, trace);
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

    pub(super) fn render(&mut self) -> bool {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor()) as f32;
        let width = self.surface_size.width as f32 / scale.max(1.0);
        let height = self.surface_size.height as f32 / scale.max(1.0);
        let list = self.display_list(width.max(1.0), height.max(1.0));
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        renderer.resize(self.surface_size.width, self.surface_size.height, scale);
        if let Err(error) = renderer.render(&list) {
            eprintln!("Sabine OSR render failed: {error}");
            return false;
        }
        if self.effect_regions_dirty {
            self.effect_regions_dirty = false;
            self.update_effect_regions();
        }
        true
    }

    pub(super) fn display_list(&self, width: f32, height: f32) -> DisplayList {
        let opaque_swapchain = self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.surface_alpha_is_opaque());
        let background = if self.config.transparent && opaque_swapchain {
            self.config.background_color
        } else if self.config.transparent {
            Color::rgba(0.0, 0.0, 0.0, 0.0)
        } else {
            self.config.background_color
        };
        let mut list = DisplayList::new(background);
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
                color: self
                    .config
                    .background_color
                    .opacity(if self.config.transparent { 0.38 } else { 1.0 }),
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
                    color: self.config.background_color,
                });
            }
        }
        self.draw_titlebar(&mut list, width);
        if self.loading.is_some_and(|loading| loading.revealed()) {
            self.draw_loading(&mut list, width, height);
            return list;
        }
        if !self.main_surface_ready() {
            return list;
        }
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
        self.draw_tooltip(&mut list, width, height);
        list
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
