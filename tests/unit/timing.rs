use super::*;

fn timings() -> Timings {
    Timings {
        minimum: Duration::from_millis(8),
        reports: BTreeMap::new(),
        external: Duration::ZERO,
        openxr: Duration::ZERO,
        gpu: Duration::ZERO,
    }
}

#[test]
fn ordinary_frame_pacing_is_not_slow_work() {
    let mut timings = timings();
    let now = Instant::now();
    let period = Duration::from_millis(11);
    assert!(
        timings
            .observe("openxr/wait-frame", period, period * 3, now)
            .is_none()
    );
    assert!(
        timings
            .observe("openxr/wait-frame", period * 4, period * 3, now)
            .is_some()
    );
    assert!(
        timings
            .observe("app/capture", period, Duration::ZERO, now)
            .is_some()
    );
}

#[test]
fn slow_warnings_are_rate_limited_per_stage() {
    let mut timings = timings();
    let now = Instant::now();
    let elapsed = Duration::from_millis(10);
    assert!(
        timings
            .observe("app/capture", elapsed, Duration::ZERO, now)
            .is_some()
    );
    assert!(
        timings
            .observe("app/capture", elapsed * 4, Duration::ZERO, now + elapsed)
            .is_none()
    );
    assert!(
        timings
            .observe("openxr/end-frame", elapsed, Duration::ZERO, now + elapsed)
            .is_some()
    );
    let warning = timings
        .observe(
            "app/capture",
            elapsed,
            Duration::ZERO,
            now + Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(warning.suppressed, 1);
    assert_eq!(warning.worst, elapsed * 4);
}

#[test]
fn external_waits_are_not_charged_to_app_work() {
    let mut timings = timings();
    timings.external = Duration::from_millis(20);
    assert_eq!(
        timings.app_elapsed(Duration::from_millis(25)),
        Duration::from_millis(5)
    );
    assert_eq!(
        timings.app_elapsed(Duration::from_millis(10)),
        Duration::ZERO
    );
    timings.reset_external();
    assert_eq!(
        timings.app_elapsed(Duration::from_millis(25)),
        Duration::from_millis(25)
    );
}

#[test]
fn individually_fast_calls_are_aggregated_by_domain() {
    let mut timings = timings();
    let elapsed = Duration::from_millis(5);
    timings.charge_external("openxr/sync-actions", elapsed);
    timings.charge_external("openxr/action-state", elapsed);
    timings.charge_external("gpu/copy-completion", elapsed);
    timings.charge_external("mixed/panel-retire", elapsed);
    timings.charge_external("app/capture", elapsed);
    assert_eq!(timings.openxr, elapsed * 2);
    assert_eq!(timings.gpu, elapsed);
    assert_eq!(timings.app_elapsed(elapsed * 5), elapsed);
    let now = Instant::now();
    assert!(
        timings
            .observe("openxr/frame-calls", timings.openxr, Duration::ZERO, now)
            .is_some()
    );
    assert!(
        timings
            .observe("gpu/frame-work", timings.gpu, Duration::ZERO, now)
            .is_none()
    );
    timings.reset_external();
    assert_eq!(timings.openxr, Duration::ZERO);
    assert_eq!(timings.gpu, Duration::ZERO);
}

#[test]
fn combined_work_can_exceed_budget_without_a_single_slow_domain() {
    let mut timings = timings();
    let elapsed = Duration::from_millis(5);
    let budget = Duration::from_millis(11);
    let now = Instant::now();
    timings.charge_external("openxr/action-state", elapsed);
    timings.charge_external("gpu/copy-completion", elapsed);
    assert!(
        timings
            .observe("openxr/frame-calls", timings.openxr, budget, now)
            .is_none()
    );
    assert!(
        timings
            .observe("gpu/frame-work", timings.gpu, budget, now)
            .is_none()
    );
    assert!(
        timings
            .observe(
                "app/xr-frame-work",
                timings.app_elapsed(elapsed * 3),
                budget,
                now
            )
            .is_none()
    );
    assert!(
        timings
            .observe("mixed/xr-active-frame", elapsed * 3, budget, now)
            .is_some()
    );
}
