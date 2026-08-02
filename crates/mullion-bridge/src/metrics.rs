//! Mullion launch metrics.
//!
//! Launch stages (`dev_command.spawned`, `host.spawned.pid.<pid>`, etc.) are
//! exposed by `MullionProcess::metrics()` for tracing and profiling.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const MULLION_TRACE_ENV: &str = "MULLION_TRACE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MullionLaunchMetric {
    pub stage: String,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MullionLaunchMetricsSnapshot {
    pub label: String,
    pub elapsed: Duration,
    pub stages: Vec<MullionLaunchMetric>,
}

#[derive(Clone, Debug)]
pub struct LaunchMetrics {
    started: Instant,
    label: String,
    trace: bool,
    stages: Arc<Mutex<Vec<MullionLaunchMetric>>>,
}

impl LaunchMetrics {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            started: Instant::now(),
            label: label.into(),
            trace: trace_enabled(),
            stages: Arc::default(),
        }
    }

    pub fn mark(&self, stage: impl Into<String>) {
        let stage = stage.into();
        let elapsed = self.started.elapsed();
        if self.trace {
            eprintln!(
                "mullion trace [{}] +{}ms {stage}",
                self.label,
                elapsed.as_millis()
            );
        }
        if let Ok(mut stages) = self.stages.lock() {
            stages.push(MullionLaunchMetric { stage, elapsed });
        }
    }

    pub fn snapshot(&self) -> MullionLaunchMetricsSnapshot {
        MullionLaunchMetricsSnapshot {
            label: self.label.clone(),
            elapsed: self.started.elapsed(),
            stages: self
                .stages
                .lock()
                .map(|stages| stages.clone())
                .unwrap_or_default(),
        }
    }
}

fn trace_enabled() -> bool {
    std::env::var(MULLION_TRACE_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "trace"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_snapshot_keeps_stage_order() {
        let metrics = LaunchMetrics::new("test");
        metrics.mark("start");
        metrics.mark("ready");
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.label, "test");
        assert_eq!(snapshot.stages[0].stage, "start");
        assert_eq!(snapshot.stages[1].stage, "ready");
    }
}
