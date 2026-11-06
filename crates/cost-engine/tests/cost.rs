//! Cost-engine tests. Tariffs here are SYNTHETIC.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::*;
use pc_core::{PowerState, NANO};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::str::FromStr;
use tariff_engine::Tariff;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn sofia(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    let tz: Tz = "Europe/Sofia".parse().unwrap();
    tz.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().with_timezone(&Utc)
}

fn ranges(v: &[(&str, &str)]) -> Value {
    Value::Array(v.iter().map(|(s, e)| json!({"start": s, "end": e})).collect())
}

fn flat(id: &str, version: u32, valid_from: &str, gross: &str) -> Tariff {
    Tariff::from_value(json!({
        "schema_version": 1, "id": id, "version": version, "name": id, "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": valid_from,
        "components": [{"id": "all", "kind": "energy", "vat_inclusive": true, "per_kwh": gross}],
        "periods": [{"id": "flat", "label": "Flat"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": ranges(&[("00:00","24:00")]), "period": "flat"}]}]}
    }))
    .unwrap()
}

/// VAT 0, night 00-06 + 22-24 at 0.10, day 06-22 at 0.20, every day, all year.
fn two_zone() -> Tariff {
    Tariff::from_value(json!({
        "schema_version": 1, "id": "tz", "name": "tz", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": "2026-01-01",
        "components": [{"id": "e", "kind": "energy", "vat_inclusive": true,
                        "per_period": {"day": "0.20", "night": "0.10"}}],
        "periods": [{"id": "day", "label": "Day"}, {"id": "night", "label": "Night"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": ranges(&[("00:00","06:00"),("22:00","24:00")]), "period": "night"},
            {"days": ["all"], "ranges": ranges(&[("06:00","22:00")]), "period": "day"}]}]}
    }))
    .unwrap()
}

fn sample(start: DateTime<Utc>, end: DateTime<Utc>, state: PowerState, watts: &str) -> UsageSample {
    UsageSample { start, end, state, watts: dec(watts), billable: true }
}

/// Per-interval rounding to nano-units is bounded by 1 nano per interval (it is ~0.33 in practice).
fn near(actual: i64, expected: i64, intervals: usize) {
    assert!((actual - expected).abs() <= intervals as i64, "{actual} vs {expected} (tolerance {intervals} nano)");
}

fn sum_kwh(v: &[Interval]) -> i64 {
    v.iter().map(|i| i.kwh_nano).sum()
}
fn sum_cost(v: &[Interval]) -> i64 {
    v.iter().map(|i| i.cost_nano).sum()
}

#[test]
fn one_hour_at_100w_flat_costs_exactly_two_cents() {
    let set = TariffSet::single(flat("f", 1, "2026-01-01", "0.20"));
    let start = sofia(2026, 5, 4, 10, 0);
    let out = price_sample(&set, &sample(start, start + Duration::hours(1), PowerState::Active, "100")).unwrap();
    assert_eq!(out.len(), 60);
    near(sum_kwh(&out), NANO / 10, 60); // 0.1 kWh
    near(sum_cost(&out), 20_000_000, 60); // 0.02 EUR
    assert_eq!(summarize(&out).total.cost_minor(2), 2); // cents are exact after display rounding
}

#[test]
fn per_minute_rounding_drift_is_bounded_by_one_nano_per_interval() {
    // 37 W for 1 h = 0.037 kWh exactly; each minute is 0.000616666... kWh and is rounded to nano.
    let set = TariffSet::single(flat("f", 1, "2026-01-01", "0.20"));
    let start = sofia(2026, 5, 4, 10, 0);
    let out = price_sample(&set, &sample(start, start + Duration::hours(1), PowerState::Idle, "37")).unwrap();
    let exact = 37_000_000i64; // nano-kWh
    assert!((sum_kwh(&out) - exact).abs() <= 60, "drift {}", sum_kwh(&out) - exact);
}

#[test]
fn span_crossing_a_period_boundary_is_split_and_priced_per_period() {
    let set = TariffSet::single(two_zone());
    let start = sofia(2026, 5, 4, 21, 30); // 30 min day, then 30 min night
    let out = price_sample(&set, &sample(start, start + Duration::hours(1), PowerState::Active, "100")).unwrap();
    let s = summarize(&out);
    // 0.05 kWh * 0.20 = 0.010 ; 0.05 kWh * 0.10 = 0.005
    near(s.by_period["day"].cost_nano, 10_000_000, 30);
    near(s.by_period["night"].cost_nano, 5_000_000, 30);
    assert_eq!(s.by_period["day"].seconds, 1800);
    assert_eq!(s.by_period["night"].seconds, 1800);
}

#[test]
fn span_crossing_local_midnight_stays_night_across_the_date_change() {
    let set = TariffSet::single(two_zone());
    let start = sofia(2026, 5, 4, 23, 30);
    let out = price_sample(&set, &sample(start, start + Duration::hours(1), PowerState::Active, "100")).unwrap();
    assert!(out.iter().all(|i| i.period_id == "night"));
}

#[test]
fn partial_minutes_at_both_ends_are_prorated_by_seconds() {
    let set = TariffSet::single(flat("f", 1, "2026-01-01", "0.20"));
    let start = sofia(2026, 5, 4, 10, 0) + Duration::seconds(30);
    let end = sofia(2026, 5, 4, 10, 1) + Duration::seconds(45);
    let out = price_sample(&set, &sample(start, end, PowerState::Idle, "60")).unwrap();
    assert_eq!(out.iter().map(|i| i.seconds).collect::<Vec<_>>(), vec![30, 45]);
    // 60 W for 75 s = 0.00125 kWh
    assert_eq!(sum_kwh(&out), 1_250_000);
}

#[test]
fn dst_spring_forward_day_is_23_real_hours() {
    // Sofia 2026-03-29 local midnight -> next local midnight is 23 h; 1 kW constant.
    let set = TariffSet::single(two_zone());
    let start = sofia(2026, 3, 29, 0, 0);
    let end = sofia(2026, 3, 30, 0, 0);
    assert_eq!(end - start, Duration::hours(23));
    let s = summarize(&price_sample(&set, &sample(start, end, PowerState::Active, "1000")).unwrap());
    // 00-06 minus skipped hour, plus 22-24 = 7 h night; 16 h day (tolerance: 1 nano per minute)
    near(s.by_period["night"].kwh_nano, 7 * NANO, 420);
    near(s.by_period["day"].kwh_nano, 16 * NANO, 960);
    assert_eq!(s.by_period["night"].seconds, 7 * 3600);
    assert_eq!(s.by_period["day"].seconds, 16 * 3600);
    near(s.total.cost_nano, 3_900_000_000, 1380); // 7*0.10 + 16*0.20 = 3.90
}

#[test]
fn dst_fall_back_day_is_25_real_hours() {
    let set = TariffSet::single(two_zone());
    let start = sofia(2026, 10, 25, 0, 0);
    let end = sofia(2026, 10, 26, 0, 0);
    assert_eq!(end - start, Duration::hours(25));
    let s = summarize(&price_sample(&set, &sample(start, end, PowerState::Active, "1000")).unwrap());
    near(s.by_period["night"].kwh_nano, 9 * NANO, 540); // repeated hour counted twice
    near(s.by_period["day"].kwh_nano, 16 * NANO, 960);
    assert_eq!(s.by_period["night"].seconds, 9 * 3600);
    assert_eq!(s.by_period["day"].seconds, 16 * 3600);
    near(s.total.cost_nano, 4_100_000_000, 1500); // 9*0.10 + 16*0.20 = 4.10
}

#[test]
fn battery_energy_keeps_kwh_but_costs_nothing() {
    let set = TariffSet::single(flat("f", 1, "2026-01-01", "0.20"));
    let start = sofia(2026, 5, 4, 10, 0);
    let mut s = sample(start, start + Duration::hours(1), PowerState::Active, "100");
    s.billable = false;
    let out = price_sample(&set, &s).unwrap();
    near(sum_kwh(&out), NANO / 10, 60);
    assert_eq!(sum_cost(&out), 0);
}

#[test]
fn invalid_samples_are_rejected() {
    let set = TariffSet::single(flat("f", 1, "2026-01-01", "0.20"));
    let t = sofia(2026, 5, 4, 10, 0);
    assert_eq!(price_sample(&set, &sample(t, t, PowerState::Idle, "10")), Err(CostError::EmptySpan));
    assert_eq!(
        price_sample(&set, &sample(t, t + Duration::minutes(1), PowerState::Idle, "-1")),
        Err(CostError::NegativeWatts)
    );
    assert_eq!(TariffSet::new(vec![]).unwrap_err(), CostError::NoTariff);
}

#[test]
fn price_change_on_july_1_applies_from_local_midnight_and_records_version() {
    let set = TariffSet::new(vec![
        flat("f", 1, "2025-07-01", "0.20"),
        flat("f", 2, "2026-07-01", "0.25"),
    ])
    .unwrap();
    let start = sofia(2026, 6, 30, 23, 30);
    let out = price_sample(&set, &sample(start, start + Duration::hours(1), PowerState::Active, "1000")).unwrap();
    for i in &out {
        let before = i.minute_start < sofia(2026, 7, 1, 0, 0);
        assert_eq!(i.tariff_version, if before { 1 } else { 2 });
        assert_eq!(i.price_nano_per_kwh, if before { 200_000_000 } else { 250_000_000 });
    }
    // 0.5 kWh * 0.20 + 0.5 kWh * 0.25
    assert_eq!(sum_cost(&out), 100_000_000 + 125_000_000);
}

#[test]
fn expired_tariff_keeps_being_used_until_replaced() {
    let mut v = json!({
        "schema_version": 1, "id": "old", "name": "old", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": "2025-07-01", "valid_to": "2026-06-30",
        "components": [{"id": "all", "kind": "energy", "vat_inclusive": true, "per_kwh": "0.20"}],
        "periods": [{"id": "flat", "label": "Flat"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": ranges(&[("00:00","24:00")]), "period": "flat"}]}]}
    });
    v["version"] = json!(1);
    let set = TariffSet::single(Tariff::from_value(v).unwrap());
    assert_eq!(set.at(sofia(2027, 2, 1, 12, 0)).id, "old");
    // and before valid_from the earliest tariff is the fallback
    assert_eq!(set.at(sofia(2024, 2, 1, 12, 0)).id, "old");
}

#[test]
fn recompute_with_corrected_tariff_changes_cost_but_not_energy() {
    let samples = vec![
        sample(sofia(2026, 5, 4, 8, 0), sofia(2026, 5, 4, 10, 0), PowerState::Active, "80"),
        sample(sofia(2026, 5, 4, 10, 0), sofia(2026, 5, 4, 12, 0), PowerState::Idle, "30"),
    ];
    let before = summarize(&price_samples(&TariffSet::single(flat("f", 1, "2026-01-01", "0.20")), &samples).unwrap());
    let after = summarize(&price_samples(&TariffSet::single(flat("f", 2, "2026-01-01", "0.30")), &samples).unwrap());
    assert_eq!(before.total.kwh_nano, after.total.kwh_nano);
    near(before.total.kwh_nano, 220_000_000, 240);
    near(before.total.cost_nano, 44_000_000, 240);
    near(after.total.cost_nano, 66_000_000, 240);
    assert!(after.total.cost_nano > before.total.cost_nano);
}

#[test]
fn summary_splits_by_state_and_period() {
    let set = TariffSet::single(two_zone());
    let samples = vec![
        sample(sofia(2026, 5, 4, 12, 0), sofia(2026, 5, 4, 13, 0), PowerState::Active, "200"),
        sample(sofia(2026, 5, 4, 23, 0), sofia(2026, 5, 5, 0, 0), PowerState::Idle, "50"),
        sample(sofia(2026, 5, 5, 0, 0), sofia(2026, 5, 5, 1, 0), PowerState::Sleep, "2"),
    ];
    let s = summarize(&price_samples(&set, &samples).unwrap());
    near(s.by_state[&PowerState::Active].cost_nano, 40_000_000, 60); // 0.2 kWh * 0.20
    near(s.by_state[&PowerState::Idle].cost_nano, 5_000_000, 60); // 0.05 kWh * 0.10
    near(s.by_state[&PowerState::Sleep].cost_nano, 200_000, 60); // 0.002 kWh * 0.10
    assert_eq!(s.by_period["night"].seconds, 7200);
    assert_eq!(s.by_period["day"].seconds, 3600);
    near(s.total.cost_nano, 45_200_000, 180);
}

#[test]
fn minor_unit_rounding_is_half_even_and_only_applied_at_the_end() {
    assert_eq!(nano_to_minor(5_000_000, 2), 0); // 0.005 -> 0.5 cent -> 0 (even)
    assert_eq!(nano_to_minor(15_000_000, 2), 2); // 1.5 cents -> 2
    assert_eq!(nano_to_minor(25_000_000, 2), 2); // 2.5 cents -> 2 (even)
    assert_eq!(nano_to_minor(1_234_000_000, 2), 123);
    let _ = NaiveDate::from_ymd_opt(2026, 1, 1); // keep chrono import used in all cfgs
}

