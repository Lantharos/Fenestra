use std::time::{Duration, Instant};

/// Tracks accelerated-paint health and decides when to silently relaunch the
/// CEF host with software OSR (`--sabine-software-osr`).
#[derive(Debug, Clone)]
pub(crate) struct AccelFallbackPolicy {
    started: Instant,
    frames: u32,
    accel_ok: u32,
    accel_fail: u32,
    software: bool,
}

impl Default for AccelFallbackPolicy {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            frames: 0,
            accel_ok: 0,
            accel_fail: 0,
            software: false,
        }
    }
}

impl AccelFallbackPolicy {
    pub(crate) fn mark_software(&mut self) {
        self.software = true;
    }

    pub(crate) fn is_software(&self) -> bool {
        self.software
    }

    pub(crate) fn note_frame(&mut self) {
        self.frames = self.frames.saturating_add(1);
    }

    pub(crate) fn note_accel_ok(&mut self) {
        self.accel_ok = self.accel_ok.saturating_add(1);
        self.accel_fail = 0;
        self.note_frame();
    }

    pub(crate) fn note_accel_fail(&mut self) {
        self.accel_fail = self.accel_fail.saturating_add(1);
    }
}

/// Relaunch into software OSR when accelerated paints never succeed after a
/// short grace window, when decode/import fails repeatedly, or when the GPU
/// process is dead and no paints arrive at all.
pub(crate) fn should_relaunch_software(policy: &AccelFallbackPolicy) -> bool {
    if policy.software {
        return false;
    }
    if policy.accel_ok > 0 {
        return policy.accel_fail >= 24;
    }
    if policy.started.elapsed() < Duration::from_secs(3) {
        return false;
    }
    policy.accel_fail >= 3 || policy.frames == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_triggers_software_fallback() {
        let mut policy = AccelFallbackPolicy::default();
        policy.started = Instant::now() - Duration::from_secs(4);
        assert!(should_relaunch_software(&policy));
    }

    #[test]
    fn frames_without_accel_fail_do_not_fallback_immediately() {
        let mut policy = AccelFallbackPolicy::default();
        policy.started = Instant::now() - Duration::from_secs(4);
        policy.note_frame();
        assert!(!should_relaunch_software(&policy));
    }
}
