use std::time::{Duration, Instant};

/// Tracks accelerated-paint health and decides when to silently relaunch the
/// CEF host with software OSR (`--sabine-software-osr`).
#[derive(Debug, Clone)]
pub(crate) struct AccelFallbackPolicy {
    started: Instant,
    accel_ok: u32,
    accel_fail: u32,
    software: bool,
}

impl Default for AccelFallbackPolicy {
    fn default() -> Self {
        Self {
            started: Instant::now(),
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

    pub(crate) fn note_accel_ok(&mut self) {
        self.accel_ok = self.accel_ok.saturating_add(1);
        self.accel_fail = 0;
    }

    pub(crate) fn note_accel_fail(&mut self) {
        self.accel_fail = self.accel_fail.saturating_add(1);
    }
}

/// Relaunch into software OSR when accelerated paints never succeed after a
/// short grace window, or when decode/import fails repeatedly.
pub(crate) fn should_relaunch_software(policy: &AccelFallbackPolicy) -> bool {
    if policy.software {
        return false;
    }
    if policy.accel_ok > 0 {
        return policy.accel_fail >= 24;
    }
    policy.started.elapsed() >= Duration::from_secs(4) && policy.accel_fail >= 3
}
