use winit::event::{ButtonSource, Force, PointerSource, TabletToolKind};

use crate::osr::host::native::OsrNativeHost;

impl OsrNativeHost {
    pub(in crate::osr::host) fn forward_pointer_source(
        &self,
        source: &PointerSource,
        phase: &str,
        x: f32,
        y: f32,
    ) -> bool {
        match source {
            PointerSource::Touch { finger_id, force } => {
                self.forward_touch(
                    touch_id(finger_id.into_raw()),
                    x,
                    y,
                    phase,
                    normalized_force(*force),
                    "touch",
                );
                true
            }
            PointerSource::TabletTool { kind, data } => {
                self.forward_touch(
                    0,
                    x,
                    y,
                    phase,
                    normalized_force(data.force),
                    pointer_type(*kind),
                );
                true
            }
            _ => false,
        }
    }

    pub(in crate::osr::host) fn forward_pointer_button(
        &self,
        button: &ButtonSource,
        phase: &str,
        x: f32,
        y: f32,
    ) -> bool {
        match button {
            ButtonSource::Touch { finger_id, force } => {
                self.forward_touch(
                    touch_id(finger_id.into_raw()),
                    x,
                    y,
                    phase,
                    normalized_force(*force),
                    "touch",
                );
                true
            }
            ButtonSource::TabletTool { kind, data, .. } => {
                self.forward_touch(
                    0,
                    x,
                    y,
                    phase,
                    normalized_force(data.force),
                    pointer_type(*kind),
                );
                true
            }
            _ => false,
        }
    }

    fn forward_touch(
        &self,
        id: i32,
        x: f32,
        y: f32,
        phase: &str,
        pressure: f32,
        pointer_type: &str,
    ) {
        let Some((x, y)) = self.content_position(x, y) else {
            return;
        };
        self.send_control(&format!(
            "touch\t{x:.2}\t{y:.2}\t{id}\t{phase}\t{pressure:.4}\t{pointer_type}\t{}\n",
            self.input_modifiers()
        ));
    }
}

fn touch_id(id: usize) -> i32 {
    (id % i32::MAX as usize) as i32
}

fn normalized_force(force: Option<Force>) -> f32 {
    force
        .map(|force| force.normalized(None).clamp(0.0, 1.0) as f32)
        .unwrap_or(0.0)
}

fn pointer_type(kind: TabletToolKind) -> &'static str {
    match kind {
        TabletToolKind::Eraser => "eraser",
        TabletToolKind::Pen
        | TabletToolKind::Brush
        | TabletToolKind::Pencil
        | TabletToolKind::Airbrush => "pen",
        _ => "unknown",
    }
}
