use chrono::{Duration, TimeZone, Utc};
use cost_engine::TariffSet;
use estimator::{Estimator, EstimatorConfig};
use pc_core::PowerState;
use simulator::*;
use std::collections::HashSet;
use storage::Storage;
use tariff_engine::Tariff;

fn est() -> Estimator {
    Estimator::new(EstimatorConfig::default()).unwrap()
}

const SOFIA: chrono_tz::Tz = chrono_tz::Europe::Sofia;

#[test]
fn spans_tile_the_requested_range_exactly() {
    let from = Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap();
    let to = from + Duration::days(3);
    let spans = generate(&SimConfig::default(), &est(), from, to, SOFIA);
    assert_eq!(spans.first().unwrap().start, from);
    assert_eq!(spans.last().unwrap().end, to);
    for w in spans.windows(2) {
        assert_eq!(w[0].end, w[1].start);
    }
}

#[test]
fn same_seed_same_data_different_seed_different_data() {
    let from = Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap();
    let to = from + Duration::days(2);
    let a = generate(&SimConfig::default(), &est(), from, to, SOFIA);
    let b = generate(&SimConfig::default(), &est(), from, to, SOFIA);
    let c = generate(&SimConfig { seed: 7, ..SimConfig::default() }, &est(), from, to, SOFIA);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn a_week_contains_every_state_and_nights_are_merged_into_long_spans() {
    let from = Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap();
    let spans = generate(&SimConfig::default(), &est(), from, from + Duration::days(14), SOFIA);
    let states: HashSet<_> = spans.iter().map(|s| s.inputs.state).collect();
    for s in [PowerState::Active, PowerState::Idle, PowerState::Sleep, PowerState::Off] {
        assert!(states.contains(&s), "{s:?} missing");
    }
    let longest_night = spans
        .iter()
        .filter(|s| matches!(s.inputs.state, PowerState::Off | PowerState::Sleep))
        .map(|s| (s.end - s.start).num_minutes())
        .max()
        .unwrap();
    assert!(longest_night > 300, "nights should be single long spans, longest {longest_night} min");
    // far fewer rows than minutes
    assert!(spans.len() < 14 * 1440);
}

#[test]
fn sensors_flag_controls_the_estimator_layer_and_battery_hours_are_unbilled() {
    let from = Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap(); // a Monday
    let to = from + Duration::days(5);
    let with = generate(&SimConfig::default(), &est(), from, to, SOFIA);
    assert!(with.iter().any(|s| s.inputs.cpu_power_w.is_some()));
    assert!(with.iter().any(|s| s.layer == estimator::Layer::Sensors));
    assert!(with.iter().any(|s| !s.billable), "laptop battery hour expected");

    let without = generate(&SimConfig { with_sensors: false, laptop: false, ..SimConfig::default() }, &est(), from, to, SOFIA);
    assert!(without.iter().all(|s| s.inputs.cpu_power_w.is_none() && s.billable));
}

#[test]
fn simulated_data_flows_through_storage_and_costs() {
    let tariff = Tariff::from_json(&std::fs::read_to_string(format!("{}/../../tariffs/bg-electrohold.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    let tariffs = TariffSet::single(tariff);
    let from = Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap();
    let to = from + Duration::days(7);
    let spans = generate(&SimConfig::default(), &est(), from, to, SOFIA);
    let mut s = Storage::open_in_memory().unwrap();
    s.record_spans(&spans, &tariffs).unwrap();
    let sum = s.summary(from, to).unwrap();
    assert_eq!(sum.total.seconds, 7 * 86_400);
    assert!(sum.total.cost_minor(2) > 0);
    assert!(sum.by_period.contains_key("day") && sum.by_period.contains_key("night"));
}
