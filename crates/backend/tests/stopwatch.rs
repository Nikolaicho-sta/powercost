//! Stopwatch tests. The tariff is SYNTHETIC: night 22-06 at 0.10, day at 0.20, VAT 0.

use backend::stopwatch::*;
use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::TariffSet;
use estimator::{Inputs, Layer};
use pc_core::PowerState;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::str::FromStr;
use storage::{Span, SpanSource, Storage};
use tariff_engine::Tariff;

const SOFIA: Tz = chrono_tz::Europe::Sofia;

fn sofia(h: u32, mi: u32, s: u32) -> DateTime<Utc> {
    SOFIA.with_ymd_and_hms(2026, 5, 4, h, mi, s).single().unwrap().with_timezone(&Utc)
}
fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}
fn ranges(v: &[(&str, &str)]) -> Value {
    Value::Array(v.iter().map(|(a, b)| json!({"start": a, "end": b})).collect())
}

fn tariff() -> Tariff {
    Tariff::from_value(json!({
        "schema_version": 1, "id": "t", "name": "Synthetic", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": "2026-01-01",
        "components": [{"id": "e", "kind": "energy", "vat_inclusive": true, "per_period": {"day": "0.20", "night": "0.10"}}],
        "periods": [{"id": "day", "label": "Day"}, {"id": "night", "label": "Night"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": ranges(&[("00:00","06:00"),("22:00","24:00")]), "period": "night"},
            {"days": ["all"], "ranges": ranges(&[("06:00","22:00")]), "period": "day"}]}]}
    }))
    .unwrap()
}

fn span(a: DateTime<Utc>, b: DateTime<Utc>, state: PowerState, watts: &str, layer: Layer, billable: bool) -> Span {
    Span { start: a, end: b, inputs: Inputs::new(state), watts: dec(watts), layer, billable, source: SpanSource::Live }
}

fn setup(spans: &[Span]) -> (Storage, TariffSet, Tariff) {
    let t = tariff();
    let set = TariffSet::single(t.clone());
    let mut s = Storage::open_in_memory().unwrap();
    s.record_spans(spans, &set).unwrap();
    (s, set, t)
}

fn near(a: i64, b: i64, tol: i64) {
    assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
}

// ---------- measurement ----------

#[test]
fn a_part_of_a_span_is_clipped_to_the_second() {
    let (s, set, t) = setup(&[span(sofia(10, 0, 0), sofia(11, 0, 0), PowerState::Active, "100", Layer::Sensors, true)]);
    // 10:15:00 - 10:45:00 = 30 min at 100 W = 0.05 kWh, daytime 0.20 -> 0.01 EUR
    let m = measure(&s, &set, &t, sofia(10, 15, 0), sofia(10, 45, 0), None).unwrap();
    assert_eq!(m.seconds, 1800);
    near(m.kwh_nano, 50_000_000, 60);
    near(m.cost_nano, 10_000_000, 60);
    assert_eq!(m.cost_minor, 1);
    assert_eq!(m.avg_watts, "100.0");
    assert_eq!((m.estimated_tail_seconds, m.untracked_seconds), (0, 0));
}

#[test]
fn odd_seconds_are_not_rounded_to_minutes() {
    let (s, set, t) = setup(&[span(sofia(10, 0, 0), sofia(11, 0, 0), PowerState::Active, "60", Layer::Default, true)]);
    // 10:00:10 - 10:02:35 = 145 s at 60 W = 0.0024166... kWh
    let m = measure(&s, &set, &t, sofia(10, 0, 10), sofia(10, 2, 35), None).unwrap();
    assert_eq!(m.seconds, 145);
    near(m.kwh_nano, 2_416_667, 5);
}

#[test]
fn an_interval_across_a_price_boundary_is_split_by_period() {
    let (s, set, t) = setup(&[span(sofia(21, 0, 0), sofia(23, 0, 0), PowerState::Active, "100", Layer::Sensors, true)]);
    let m = measure(&s, &set, &t, sofia(21, 30, 0), sofia(22, 30, 0), None).unwrap();
    let day = m.by_period.iter().find(|p| p.id == "day").unwrap();
    let night = m.by_period.iter().find(|p| p.id == "night").unwrap();
    assert_eq!((day.seconds, night.seconds), (1800, 1800));
    near(day.cost_nano, 10_000_000, 60); // 0.05 kWh * 0.20
    near(night.cost_nano, 5_000_000, 60); // 0.05 kWh * 0.10
    near(m.cost_nano, 15_000_000, 120);
}

#[test]
fn states_are_broken_down_and_battery_costs_nothing() {
    let (s, set, t) = setup(&[
        span(sofia(10, 0, 0), sofia(10, 30, 0), PowerState::Active, "100", Layer::Sensors, true),
        span(sofia(10, 30, 0), sofia(11, 0, 0), PowerState::Idle, "40", Layer::Sensors, false), // on battery
    ]);
    let m = measure(&s, &set, &t, sofia(10, 0, 0), sofia(11, 0, 0), None).unwrap();
    assert_eq!(m.by_state.len(), 2);
    let idle = m.by_state.iter().find(|l| l.state == PowerState::Idle).unwrap();
    assert!(idle.kwh_nano > 0 && idle.cost_nano == 0, "battery energy is counted but not billed");
    near(m.kwh_nano, 70_000_000, 100); // 0.05 + 0.02 kWh
    near(m.cost_nano, 10_000_000, 60); // only the 0.05 kWh on mains
    assert_eq!(m.avg_watts, "70.0");
}

#[test]
fn the_unstored_last_minute_is_filled_from_the_live_estimate_and_reported() {
    let (s, set, t) = setup(&[span(sofia(10, 0, 0), sofia(10, 30, 0), PowerState::Active, "100", Layer::Sensors, true)]);
    let tail = Tail { watts: dec("60"), state: PowerState::Active, billable: true };
    let m = measure(&s, &set, &t, sofia(10, 0, 0), sofia(10, 40, 0), Some(tail)).unwrap();
    assert_eq!(m.seconds, 2400);
    assert_eq!(m.estimated_tail_seconds, 600);
    assert_eq!(m.untracked_seconds, 0);
    near(m.kwh_nano, 50_000_000 + 10_000_000, 100); // 0.05 + 10 min * 60 W
    // without a tail the same interval simply has untracked time
    let m2 = measure(&s, &set, &t, sofia(10, 0, 0), sofia(10, 40, 0), None).unwrap();
    assert_eq!((m2.estimated_tail_seconds, m2.untracked_seconds), (0, 600));
    near(m2.kwh_nano, 50_000_000, 60);
}

#[test]
fn a_gap_in_the_data_is_reported_not_invented() {
    let (s, set, t) = setup(&[
        span(sofia(10, 0, 0), sofia(10, 10, 0), PowerState::Active, "100", Layer::Sensors, true),
        span(sofia(10, 20, 0), sofia(10, 30, 0), PowerState::Active, "100", Layer::Sensors, true),
    ]);
    let m = measure(&s, &set, &t, sofia(10, 0, 0), sofia(10, 30, 0), None).unwrap();
    assert_eq!(m.untracked_seconds, 600);
    near(m.kwh_nano, 33_333_333, 100); // 20 min of data only
}

#[test]
fn empty_and_zero_length_intervals_are_zero() {
    let (s, set, t) = setup(&[]);
    let m = measure(&s, &set, &t, sofia(10, 0, 0), sofia(10, 5, 0), None).unwrap();
    assert_eq!((m.kwh_nano, m.cost_nano, m.cost_minor), (0, 0, 0));
    assert_eq!(m.avg_watts, "0.0");
    assert_eq!(m.untracked_seconds, 300);
    let z = measure(&s, &set, &t, sofia(10, 0, 0), sofia(10, 0, 0), None).unwrap();
    assert_eq!((z.seconds, z.untracked_seconds), (0, 0));
}

#[test]
fn the_confidence_mix_follows_the_estimator_layers_and_adds_to_1000() {
    let (s, set, t) = setup(&[
        span(sofia(10, 0, 0), sofia(10, 30, 0), PowerState::Active, "100", Layer::Sensors, true),
        span(sofia(10, 30, 0), sofia(11, 0, 0), PowerState::Active, "100", Layer::Default, true),
    ]);
    let m = measure(&s, &set, &t, sofia(10, 0, 0), sofia(11, 0, 0), None).unwrap();
    assert_eq!((m.measured_sensors_permille, m.calibrated_permille, m.default_model_permille), (500, 0, 500));
    let none = measure(&s, &set, &t, sofia(15, 0, 0), sofia(15, 5, 0), None).unwrap();
    assert_eq!(none.measured_sensors_permille + none.calibrated_permille + none.default_model_permille, 0);
}

// ---------- lifecycle ----------

#[test]
fn start_stop_saves_a_session_with_the_label_and_clears_the_running_state() {
    let (s, set, t) = setup(&[span(sofia(10, 0, 0), sofia(11, 0, 0), PowerState::Active, "100", Layer::Sensors, true)]);
    assert!(running(&s).unwrap().is_none());
    let r = start(&s, "  Gaming session  ", sofia(10, 10, 0)).unwrap();
    assert_eq!(r.label, "Gaming session");
    assert_eq!(running(&s).unwrap().unwrap().start, sofia(10, 10, 0));
    assert!(start(&s, "again", sofia(10, 11, 0)).is_err(), "only one stopwatch at a time");

    let session = stop(&s, &set, &t, None, sofia(10, 40, 0)).unwrap();
    assert_eq!(session.label, "Gaming session");
    assert_eq!(session.result.seconds, 1800);
    near(session.result.kwh_nano, 50_000_000, 60);
    assert!(running(&s).unwrap().is_none());
    assert_eq!(history(&s).unwrap().len(), 1);
    assert!(stop(&s, &set, &t, None, sofia(10, 41, 0)).is_err(), "not running any more");
}

#[test]
fn the_running_state_survives_a_restart_of_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pc.db");
    {
        let s = Storage::open(&path).unwrap();
        start(&s, "overnight", sofia(22, 0, 0)).unwrap();
    }
    let s = Storage::open(&path).unwrap();
    let r = running(&s).unwrap().expect("still running after reopening");
    assert_eq!((r.label.as_str(), r.start), ("overnight", sofia(22, 0, 0)));
}

#[test]
fn history_is_newest_first_deletable_and_capped() {
    let (s, set, t) = setup(&[]);
    for i in 0..55 {
        start(&s, &format!("run {i}"), sofia(10, 0, 0) + Duration::minutes(i)).unwrap();
        stop(&s, &set, &t, None, sofia(10, 0, 0) + Duration::minutes(i) + Duration::seconds(30)).unwrap();
    }
    let h = history(&s).unwrap();
    assert_eq!(h.len(), 50, "oldest sessions are dropped");
    assert_eq!(h[0].label, "run 54");
    let id = h[3].id.clone();
    delete(&s, &id).unwrap();
    assert_eq!(history(&s).unwrap().len(), 49);
    assert!(history(&s).unwrap().iter().all(|x| x.id != id));
}

#[test]
fn cancel_discards_without_saving_and_a_stop_before_start_is_not_negative() {
    let (s, set, t) = setup(&[]);
    start(&s, "x", sofia(10, 0, 0)).unwrap();
    cancel(&s).unwrap();
    assert!(running(&s).unwrap().is_none());
    assert!(history(&s).unwrap().is_empty());
    // a clock that went backwards must not produce a negative interval
    start(&s, "y", sofia(10, 0, 0)).unwrap();
    let session = stop(&s, &set, &t, None, sofia(9, 59, 0)).unwrap();
    assert_eq!(session.result.seconds, 0);
}
