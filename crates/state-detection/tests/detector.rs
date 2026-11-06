use pc_core::PowerState::{Active, Idle};
use state_detection::*;

fn obs(input: Option<u64>, cpu: f64, gpu: Option<f64>) -> Observation {
    Observation { input_idle_secs: input, cpu_percent: cpu, gpu_percent: gpu }
}

#[test]
fn recent_input_is_active_even_with_zero_load() {
    let mut d = StateDetector::new(Thresholds::default());
    assert_eq!(d.update(obs(Some(5), 0.0, None)), Active);
    assert_eq!(d.update(obs(Some(119), 0.0, None)), Active);
}

#[test]
fn no_input_and_low_load_is_idle() {
    let mut d = StateDetector::new(Thresholds::default());
    assert_eq!(d.update(obs(Some(120), 3.0, Some(1.0))), Idle);
    assert_eq!(d.update(obs(Some(10_000), 0.0, None)), Idle);
}

#[test]
fn unknown_input_time_does_not_count_as_activity() {
    let mut d = StateDetector::new(Thresholds::default());
    assert_eq!(d.update(obs(None, 1.0, None)), Idle);
}

#[test]
fn heavy_cpu_load_without_input_is_active() {
    // e.g. a render or compile while the user is away
    let mut d = StateDetector::new(Thresholds::default());
    assert_eq!(d.update(obs(Some(3600), 80.0, None)), Active);
}

#[test]
fn gpu_load_alone_activates() {
    let mut d = StateDetector::new(Thresholds::default());
    assert_eq!(d.update(obs(Some(3600), 2.0, Some(60.0))), Active);
}

#[test]
fn hysteresis_keeps_active_between_exit_and_enter_levels() {
    let mut d = StateDetector::new(Thresholds::default()); // enter 20, exit 10
    assert_eq!(d.update(obs(Some(3600), 25.0, None)), Active);
    // 15% is below enter (20) but above exit (10): stays active, indefinitely
    for _ in 0..10 {
        assert_eq!(d.update(obs(Some(3600), 15.0, None)), Active);
    }
}

#[test]
fn leaving_load_activity_needs_consecutive_low_ticks() {
    let mut d = StateDetector::new(Thresholds::default()); // debounce 3
    d.update(obs(Some(3600), 50.0, None));
    assert_eq!(d.update(obs(Some(3600), 2.0, None)), Active); // 1
    assert_eq!(d.update(obs(Some(3600), 2.0, None)), Active); // 2
    assert_eq!(d.update(obs(Some(3600), 2.0, None)), Idle); // 3 -> idle
}

#[test]
fn a_blip_above_exit_level_resets_the_debounce() {
    let mut d = StateDetector::new(Thresholds::default());
    d.update(obs(Some(3600), 50.0, None));
    d.update(obs(Some(3600), 2.0, None));
    d.update(obs(Some(3600), 2.0, None));
    assert_eq!(d.update(obs(Some(3600), 12.0, None)), Active); // above exit: resets
    d.update(obs(Some(3600), 2.0, None));
    d.update(obs(Some(3600), 2.0, None));
    assert_eq!(d.update(obs(Some(3600), 2.0, None)), Idle);
}

#[test]
fn input_overrides_debounce_and_cpu_is_clamped() {
    let mut d = StateDetector::new(Thresholds::default());
    assert_eq!(d.update(obs(Some(1), 250.0, None)), Active); // "processor utility" can exceed 100
    assert_eq!(d.update(obs(Some(1), 0.0, None)), Active); // input keeps it active
}

#[test]
fn thresholds_are_configurable() {
    let cfg = Thresholds { input_idle_secs: 10, cpu_enter_percent: 50, ..Thresholds::default() };
    let mut d = StateDetector::new(cfg);
    assert_eq!(d.update(obs(Some(11), 40.0, None)), Idle);
    assert_eq!(d.update(obs(Some(9), 0.0, None)), Active);
    d.set_thresholds(Thresholds { input_idle_secs: 5, ..cfg });
    assert_eq!(d.update(obs(Some(9), 0.0, None)), Idle);
}
