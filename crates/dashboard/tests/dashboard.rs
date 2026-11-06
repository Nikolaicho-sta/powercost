//! Dashboard tests. Tariffs are SYNTHETIC (night 22-06 at 0.10, day at 0.20, VAT 0).

use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::TariffSet;
use dashboard::*;
use estimator::{Estimator, EstimatorConfig, Inputs, Layer};
use pc_core::PowerState;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::str::FromStr;
use storage::{Span, SpanSource, Storage};
use tariff_engine::Tariff;

const SOFIA: Tz = chrono_tz::Europe::Sofia;

fn sofia(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    SOFIA.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().with_timezone(&Utc)
}

fn ranges(v: &[(&str, &str)]) -> Value {
    Value::Array(v.iter().map(|(s, e)| json!({"start": s, "end": e})).collect())
}

fn tariff_json(valid_to: Option<&str>) -> Value {
    json!({
        "schema_version": 1, "id": "t", "version": 1, "name": "Synthetic", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": "2026-01-01", "valid_to": valid_to,
        "last_verified": "2026-10-01",
        "components": [{"id": "e", "kind": "energy", "vat_inclusive": true, "per_period": {"day": "0.20", "night": "0.10"}}],
        "periods": [{"id": "day", "label": "Day"}, {"id": "night", "label": "Night"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": ranges(&[("00:00","06:00"),("22:00","24:00")]), "period": "night"},
            {"days": ["all"], "ranges": ranges(&[("06:00","22:00")]), "period": "day"}]}]}
    })
}

fn tariff() -> Tariff {
    Tariff::from_value(tariff_json(None)).unwrap()
}

fn span(a: DateTime<Utc>, b: DateTime<Utc>, state: PowerState, watts: &str, billable: bool) -> Span {
    Span {
        start: a,
        end: b,
        inputs: Inputs::new(state),
        watts: Decimal::from_str(watts).unwrap(),
        layer: Layer::Default,
        billable,
        source: SpanSource::Simulated,
    }
}

fn near(a: i64, b: i64, tol: i64) {
    assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
}

// ---------- ranges ----------

#[test]
fn today_range_follows_local_midnight_and_dst() {
    let r = range_for(PeriodKind::Today, sofia(2026, 3, 29, 12, 0), SOFIA);
    assert_eq!(r.from, sofia(2026, 3, 29, 0, 0));
    assert_eq!(r.period_end - r.from, Duration::hours(23)); // spring forward
    assert_eq!(r.to, sofia(2026, 3, 29, 12, 0));
    let r = range_for(PeriodKind::Today, sofia(2026, 10, 25, 12, 0), SOFIA);
    assert_eq!(r.period_end - r.from, Duration::hours(25)); // fall back
}

#[test]
fn week_starts_monday_month_and_year_roll_over() {
    let wed = sofia(2026, 11, 4, 15, 0); // Wednesday
    let w = range_for(PeriodKind::Week, wed, SOFIA);
    assert_eq!(w.from, sofia(2026, 11, 2, 0, 0));
    assert_eq!(w.period_end, sofia(2026, 11, 9, 0, 0));
    let sun = range_for(PeriodKind::Week, sofia(2026, 11, 8, 23, 0), SOFIA);
    assert_eq!(sun.from, sofia(2026, 11, 2, 0, 0));

    let m = range_for(PeriodKind::Month, sofia(2026, 12, 20, 9, 0), SOFIA);
    assert_eq!(m.from, sofia(2026, 12, 1, 0, 0));
    assert_eq!(m.period_end, sofia(2027, 1, 1, 0, 0));

    let y = range_for(PeriodKind::Year, wed, SOFIA);
    assert_eq!(y.from, sofia(2026, 1, 1, 0, 0));
    assert_eq!(y.period_end, sofia(2027, 1, 1, 0, 0));
}

// ---------- dashboard ----------

fn sim_store(days: i64, until: DateTime<Utc>) -> (Storage, Tariff) {
    let tariff = tariff();
    let est = Estimator::new(EstimatorConfig::default()).unwrap();
    let from = until - Duration::days(days);
    let spans = simulator::generate(&simulator::SimConfig::default(), &est, from, until, SOFIA);
    let mut s = Storage::open_in_memory().unwrap();
    s.record_spans(&spans, &TariffSet::single(tariff.clone())).unwrap();
    (s, tariff)
}

fn sum_amounts<'a>(it: impl Iterator<Item = &'a Amount>) -> Amount {
    it.fold(Amount::default(), |a, b| Amount {
        seconds: a.seconds + b.seconds,
        kwh_nano: a.kwh_nano + b.kwh_nano,
        cost_nano: a.cost_nano + b.cost_nano,
        cost_minor: 0, // sums of rounded cents are not compared
    })
}

fn key(a: Amount) -> (i64, i64, i64) {
    (a.seconds, a.kwh_nano, a.cost_nano)
}

#[test]
fn breakdowns_and_timeline_add_up_to_the_total() {
    let now = sofia(2026, 11, 20, 12, 0);
    let (s, t) = sim_store(70, now); // covers Nov 1..20 and all of October
    for kind in [PeriodKind::Today, PeriodKind::Week, PeriodKind::Month, PeriodKind::Year] {
        let d = dashboard(&s, &t, kind, now, None).unwrap();
        assert!(d.total.cost_nano > 0, "{kind:?}");
        assert_eq!(key(sum_amounts(d.by_state.iter().map(|r| &r.amount))), key(d.total), "{kind:?} by_state");
        assert_eq!(key(sum_amounts(d.by_period.iter().map(|r| &r.amount))), key(d.total), "{kind:?} by_period");
        assert_eq!(key(sum_amounts(d.timeline.iter().map(|p| &p.total))), key(d.total), "{kind:?} timeline");
        assert_eq!(d.currency, "EUR");
    }
}

#[test]
fn timeline_has_a_bucket_for_every_hour_day_or_month_including_future_ones() {
    let now = sofia(2026, 11, 20, 12, 0);
    let (s, t) = sim_store(30, now);
    let today = dashboard(&s, &t, PeriodKind::Today, now, None).unwrap();
    assert_eq!((today.bucket.as_str(), today.timeline.len()), ("hour", 24));
    assert_eq!(today.timeline.iter().filter(|p| p.future).count(), 12);
    assert_eq!(dashboard(&s, &t, PeriodKind::Week, now, None).unwrap().timeline.len(), 7);
    assert_eq!(dashboard(&s, &t, PeriodKind::Month, now, None).unwrap().timeline.len(), 30);
    let year = dashboard(&s, &t, PeriodKind::Year, now, None).unwrap();
    assert_eq!((year.bucket.as_str(), year.timeline.len()), ("month", 12));
}

#[test]
fn dst_days_have_23_and_25_hourly_buckets() {
    let (s, t) = sim_store(3, sofia(2026, 3, 29, 20, 0));
    let d = dashboard(&s, &t, PeriodKind::Today, sofia(2026, 3, 29, 20, 0), None).unwrap();
    assert_eq!(d.timeline.len(), 23);
    let (s, t) = sim_store(3, sofia(2026, 10, 25, 20, 0));
    let d = dashboard(&s, &t, PeriodKind::Today, sofia(2026, 10, 25, 20, 0), None).unwrap();
    assert_eq!(d.timeline.len(), 25);
}

#[test]
fn compaction_does_not_change_month_and_year_figures() {
    let now = sofia(2026, 11, 20, 12, 0);
    let (mut s, t) = sim_store(70, now);
    let before_m = dashboard(&s, &t, PeriodKind::Month, now, None).unwrap();
    let before_y = dashboard(&s, &t, PeriodKind::Year, now, None).unwrap();
    s.compact(sofia(2026, 11, 10, 12, 0), SOFIA).unwrap();
    let after_m = dashboard(&s, &t, PeriodKind::Month, now, None).unwrap();
    let after_y = dashboard(&s, &t, PeriodKind::Year, now, None).unwrap();
    assert_eq!(before_m.total, after_m.total);
    assert_eq!(before_y.total, after_y.total);
    assert_eq!(before_m.by_state, after_m.by_state);
    // daily bars for the compacted part still exist and match
    let strip = |d: &Dashboard| d.timeline.iter().map(|p| p.total).collect::<Vec<_>>();
    assert_eq!(strip(&before_m), strip(&after_m));
    assert_eq!(strip(&before_y), strip(&after_y));
}

#[test]
fn confidence_mix_adds_to_1000_and_is_empty_without_data() {
    let now = sofia(2026, 11, 20, 12, 0);
    let (s, t) = sim_store(10, now);
    let c = dashboard(&s, &t, PeriodKind::Week, now, None).unwrap().confidence;
    assert_eq!(c.measured_sensors_permille + c.calibrated_permille + c.default_model_permille, 1000);
    assert!(c.measured_sensors_permille > 0);

    let empty = Storage::open_in_memory().unwrap();
    let d = dashboard(&empty, &t, PeriodKind::Week, now, None).unwrap();
    assert_eq!(d.confidence, ConfidenceMix::default());
    assert_eq!(key(d.total), (0, 0, 0));
    assert_eq!(d.timeline.len(), 7);
}

#[test]
fn stale_tariff_produces_warnings() {
    let expired = Tariff::from_value(tariff_json(Some("2026-06-30"))).unwrap();
    let s = Storage::open_in_memory().unwrap();
    let d = dashboard(&s, &expired, PeriodKind::Today, sofia(2026, 11, 20, 12, 0), None).unwrap();
    assert!(d.warnings.iter().any(|w| w.code == "expired"));
    let ok = dashboard(&s, &tariff(), PeriodKind::Today, sofia(2026, 11, 20, 12, 0), None).unwrap();
    assert!(ok.warnings.is_empty());
}

#[test]
fn dashboard_serialises_to_json_for_the_ui() {
    let (s, t) = sim_store(3, sofia(2026, 11, 20, 12, 0));
    let d = dashboard(&s, &t, PeriodKind::Today, sofia(2026, 11, 20, 12, 0), None).unwrap();
    let v: Value = serde_json::to_value(&d).unwrap();
    assert_eq!(v["period"], "today");
    assert!(v["total"]["cost_nano"].is_i64());
    assert!(v["timeline"][0]["local_start"].is_string());
    assert!(v["by_state"][0]["state"].is_string());
}

// ---------- what-if ----------

fn store_with(spans: &[Span]) -> Storage {
    let mut s = Storage::open_in_memory().unwrap();
    s.record_spans(spans, &TariffSet::single(tariff())).unwrap();
    s
}

fn day_range() -> (DateTime<Utc>, DateTime<Utc>) {
    (sofia(2026, 11, 1, 0, 0), sofia(2026, 11, 8, 0, 0))
}

#[test]
fn shutdown_at_night_saves_idle_energy_inside_the_window_only() {
    let s = store_with(&[
        span(sofia(2026, 11, 3, 23, 0), sofia(2026, 11, 4, 5, 0), PowerState::Idle, "50", true), // in window
        span(sofia(2026, 11, 4, 12, 0), sofia(2026, 11, 4, 14, 0), PowerState::Idle, "50", true), // outside
        span(sofia(2026, 11, 4, 23, 0), sofia(2026, 11, 5, 5, 0), PowerState::Active, "90", true), // active: not counted
        span(sofia(2026, 11, 5, 23, 0), sofia(2026, 11, 6, 5, 0), PowerState::Idle, "50", false), // battery: not billed
    ]);
    let p = WhatIfParams { night_start_hour: 23, night_end_hour: 5, off_watts: Decimal::ONE, shift_percent: 0 };
    let (a, b) = day_range();
    let r = what_if(&s, &tariff(), &p, a, b).unwrap();
    // 6 h * 50 W = 0.3 kWh at night price 0.10 = 0.03 EUR ; off: 6 h * 1 W = 0.006 kWh -> 0.0006 EUR
    near(r.shutdown_at_night.current_cost_nano, 30_000_000, 400);
    near(r.shutdown_at_night.new_cost_nano, 600_000, 400);
    near(r.shutdown_at_night.saving_nano, 29_400_000, 800);
    near(r.shutdown_at_night.kwh_saved_nano, 294_000_000, 400);
    assert_eq!(r.shutdown_at_night.hours_affected, 6);
}

#[test]
fn shutdown_window_can_wrap_midnight_and_off_watts_zero_saves_everything() {
    let s = store_with(&[span(sofia(2026, 11, 3, 23, 0), sofia(2026, 11, 4, 1, 0), PowerState::Sleep, "20", true)]);
    let p = WhatIfParams { night_start_hour: 22, night_end_hour: 6, off_watts: Decimal::ZERO, shift_percent: 0 };
    let (a, b) = day_range();
    let r = what_if(&s, &tariff(), &p, a, b).unwrap();
    assert_eq!(r.shutdown_at_night.new_cost_nano, 0);
    assert_eq!(r.shutdown_at_night.saving_nano, r.shutdown_at_night.current_cost_nano);
    near(r.shutdown_at_night.current_cost_nano, 4_000_000, 200); // 0.04 kWh * 0.10
}

#[test]
fn shifting_heavy_daytime_use_to_the_cheap_period() {
    let s = store_with(&[
        span(sofia(2026, 11, 3, 10, 0), sofia(2026, 11, 3, 12, 0), PowerState::Active, "100", true), // day price .20
        span(sofia(2026, 11, 3, 23, 0), sofia(2026, 11, 4, 1, 0), PowerState::Active, "100", true), // already cheap
    ]);
    let (a, b) = day_range();
    let half = what_if(&s, &tariff(), &WhatIfParams { shift_percent: 50, ..Default::default() }, a, b).unwrap();
    assert_eq!(half.cheapest_period, "Night");
    // 0.2 kWh at .20 = 0.04 ; moving 50% saves 0.1 kWh * (0.20 - 0.10) = 0.01
    near(half.shift_to_cheap_period.current_cost_nano, 40_000_000, 200);
    near(half.shift_to_cheap_period.saving_nano, 10_000_000, 200);
    let all = what_if(&s, &tariff(), &WhatIfParams { shift_percent: 100, ..Default::default() }, a, b).unwrap();
    near(all.shift_to_cheap_period.saving_nano, 20_000_000, 400);
    let none = what_if(&s, &tariff(), &WhatIfParams { shift_percent: 0, ..Default::default() }, a, b).unwrap();
    assert_eq!(none.shift_to_cheap_period.saving_nano, 0);
}

#[test]
fn what_if_on_empty_data_is_all_zero_and_notes_compaction() {
    let mut s = store_with(&[span(sofia(2026, 11, 1, 10, 0), sofia(2026, 11, 1, 11, 0), PowerState::Active, "100", true)]);
    let r = what_if(&s, &tariff(), &WhatIfParams::default(), sofia(2026, 12, 1, 0, 0), sofia(2026, 12, 8, 0, 0)).unwrap();
    assert_eq!(r.shutdown_at_night, Scenario::default());
    assert!(r.note.is_empty());
    s.compact(sofia(2026, 11, 5, 0, 0), SOFIA).unwrap();
    let r = what_if(&s, &tariff(), &WhatIfParams::default(), sofia(2026, 10, 1, 0, 0), sofia(2026, 12, 1, 0, 0)).unwrap();
    assert!(r.note.contains("compacted"));
}

