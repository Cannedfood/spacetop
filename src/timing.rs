use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(crate) struct Timings {
    minimum: Duration,
    reports: BTreeMap<&'static str, Report>,
    external: Duration,
    openxr: Duration,
    gpu: Duration,
}

struct Report {
    last: Instant,
    suppressed: u64,
    worst: Duration,
}

#[derive(Debug)]
struct Warning {
    elapsed: Duration,
    limit: Duration,
    suppressed: u64,
    worst: Duration,
}

impl Timings {
    pub(crate) fn new() -> Self {
        let minimum = std::env::var("SPACETOP_SLOW_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(8);
        Self {
            minimum: Duration::from_millis(minimum),
            reports: BTreeMap::new(),
            external: Duration::ZERO,
            openxr: Duration::ZERO,
            gpu: Duration::ZERO,
        }
    }

    pub(crate) fn measure<T>(
        &mut self,
        stage: &'static str,
        budget: Duration,
        operation: impl FnOnce() -> T,
    ) -> T {
        let started = Instant::now();
        let result = operation();
        let elapsed = started.elapsed();
        self.charge_external(stage, elapsed);
        self.record(stage, elapsed, budget);
        result
    }

    pub(crate) fn reset_external(&mut self) {
        self.external = Duration::ZERO;
        self.openxr = Duration::ZERO;
        self.gpu = Duration::ZERO;
    }

    fn charge_external(&mut self, stage: &'static str, elapsed: Duration) {
        if stage.starts_with("openxr/") {
            self.openxr += elapsed;
            self.external += elapsed;
        } else if stage.starts_with("gpu/") {
            self.gpu += elapsed;
            self.external += elapsed;
        } else if stage.starts_with("mixed/") {
            self.external += elapsed;
        }
    }

    pub(crate) fn record_external(
        &mut self,
        openxr_stage: &'static str,
        gpu_stage: &'static str,
        budget: Duration,
    ) {
        self.record(openxr_stage, self.openxr, budget);
        self.record(gpu_stage, self.gpu, budget);
    }

    pub(crate) fn app_elapsed(&self, elapsed: Duration) -> Duration {
        elapsed.saturating_sub(self.external)
    }

    pub(crate) fn record_frame(&mut self, elapsed: Duration, budget: Duration) {
        if self.record("mixed/xr-active-frame", elapsed, budget) {
            eprintln!(
                "[spacetop timing] frame breakdown: app {:.2} ms, OpenXR calls {:.2} ms, GPU calls/waits {:.2} ms",
                self.app_elapsed(elapsed).as_secs_f64() * 1000.0,
                self.openxr.as_secs_f64() * 1000.0,
                self.gpu.as_secs_f64() * 1000.0
            );
        }
    }

    pub(crate) fn record(
        &mut self,
        stage: &'static str,
        elapsed: Duration,
        budget: Duration,
    ) -> bool {
        if let Some(warning) = self.observe(stage, elapsed, budget, Instant::now()) {
            eprintln!(
                "[spacetop timing] {stage}: {:.2} ms (limit {:.2} ms; {} suppressed; worst {:.2} ms)",
                warning.elapsed.as_secs_f64() * 1000.0,
                warning.limit.as_secs_f64() * 1000.0,
                warning.suppressed,
                warning.worst.as_secs_f64() * 1000.0
            );
            true
        } else {
            false
        }
    }

    fn observe(
        &mut self,
        stage: &'static str,
        elapsed: Duration,
        budget: Duration,
        now: Instant,
    ) -> Option<Warning> {
        let limit = budget.max(self.minimum);
        if elapsed <= limit {
            return None;
        }
        if let Some(report) = self.reports.get_mut(stage) {
            report.worst = report.worst.max(elapsed);
            if now.duration_since(report.last) < Duration::from_secs(1) {
                report.suppressed += 1;
                return None;
            }
            let warning = Warning {
                elapsed,
                limit,
                suppressed: report.suppressed,
                worst: report.worst,
            };
            *report = Report {
                last: now,
                suppressed: 0,
                worst: Duration::ZERO,
            };
            Some(warning)
        } else {
            self.reports.insert(
                stage,
                Report {
                    last: now,
                    suppressed: 0,
                    worst: Duration::ZERO,
                },
            );
            Some(Warning {
                elapsed,
                limit,
                suppressed: 0,
                worst: elapsed,
            })
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/timing.rs"]
mod tests;
