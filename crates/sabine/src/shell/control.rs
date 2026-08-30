#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellSurfaceFrameRate(u32);

impl ShellSurfaceFrameRate {
    pub const MIN: u32 = 1;
    pub const MAX: u32 = 1_000;

    pub fn new(frame_rate: u32) -> Option<Self> {
        (Self::MIN..=Self::MAX)
            .contains(&frame_rate)
            .then_some(Self(frame_rate))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl crate::SabineProcess {
    pub fn set_shell_surface_frame_rate(&self, frame_rate: ShellSurfaceFrameRate) -> bool {
        self.bridge_emitter.as_ref().is_some_and(|emitter| {
            emitter.emit_host_control("shell-frame-rate", &frame_rate.get().to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ShellSurfaceFrameRate;

    #[test]
    fn rejects_invalid_frame_rates() {
        assert_eq!(ShellSurfaceFrameRate::new(0), None);
        assert_eq!(
            ShellSurfaceFrameRate::new(60).map(|rate| rate.get()),
            Some(60)
        );
        assert_eq!(ShellSurfaceFrameRate::new(1_001), None);
    }
}
