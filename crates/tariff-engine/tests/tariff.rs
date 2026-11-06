//! Tariff-engine tests. All fixtures here are SYNTHETIC (made-up prices and hours);
//! they test the engine, not any real tariff.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::str::FromStr;
use tariff_engine::{Tariff, TariffError, Warning};

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Local wall-clock time in `tz` -> UTC instant (must be unambiguous).
fn local(tz: &str, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    let tz: Tz = tz.parse().unwrap();
    tz.with_ymd_and_hms(y, mo, d, h, mi, 0).single().expect("unambiguous local time").with_timezone(&Utc)
}

fn sofia(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    local("Europe/Sofia", y, mo, d, h, mi)
}

fn ranges(v: &[(&str, &str)]) -> Value {
    Value::Array(v.iter().map(|(s, e)| json!({"start": s, "end": e})).collect())
}

/// Synthetic two-zone tariff: winter night 22-06, summer night 23-07 (split at midnight),
/// weekends and holiday 2026-03-03 all night.
fn two_zone() -> Value {
    json!({
        "schema_version": 1,
        "id": "test-two-zone",
        "name": "Synthetic two-zone",
        "country": "BG",
        "currency": "EUR",
        "timezone": "Europe/Sofia",
        "vat_percent": "20",
        "valid_from": "2026-07-01",
        "valid_to": "2027-06-30",
        "last_verified": "2026-11-01",
        "verification": "example",
        "components": [
            {"id": "energy", "kind": "energy", "vat_inclusive": false,
             "per_period": {"day": "0.10", "night": "0.05"}},
            {"id": "network", "kind": "network", "vat_inclusive": false, "per_kwh": "0.03"}
        ],
        "fixed_fees": [{"id": "meter", "per_month": "1.00", "vat_inclusive": false}],
        "periods": [{"id": "day", "label": "Day"}, {"id": "night", "label": "Night"}],
        "holidays": ["2026-03-03"],
        "schedule": {"seasons": [
            {"id": "winter", "from": "11-01", "to": "03-31", "rules": [
                {"days": ["weekday"], "ranges": ranges(&[("00:00","06:00"),("22:00","24:00")]), "period": "night"},
                {"days": ["weekday"], "ranges": ranges(&[("06:00","22:00")]), "period": "day"},
                {"days": ["weekend","holiday"], "ranges": ranges(&[("00:00","24:00")]), "period": "night"}
            ]},
            {"id": "summer", "from": "04-01", "to": "10-31", "rules": [
                {"days": ["weekday"], "ranges": ranges(&[("00:00","07:00"),("23:00","24:00")]), "period": "night"},
                {"days": ["weekday"], "ranges": ranges(&[("07:00","23:00")]), "period": "day"},
                {"days": ["weekend","holiday"], "ranges": ranges(&[("00:00","24:00")]), "period": "night"}
            ]}
        ]}
    })
}

fn load(v: Value) -> Tariff {
    Tariff::from_value(v).expect("fixture should be valid")
}

fn period_at(t: &Tariff, when: DateTime<Utc>) -> String {
    t.resolve(when).period_id.to_string()
}

// ---------- boundaries ----------

#[test]
fn winter_weekday_boundaries_are_start_inclusive_end_exclusive() {
    let t = load(two_zone());
    // Tue 2026-02-10
    assert_eq!(period_at(&t, sofia(2026, 2, 10, 5, 59)), "night");
    assert_eq!(period_at(&t, sofia(2026, 2, 10, 6, 0)), "day");
    assert_eq!(period_at(&t, sofia(2026, 2, 10, 21, 59)), "day");
    assert_eq!(period_at(&t, sofia(2026, 2, 10, 22, 0)), "night");
}

#[test]
fn midnight_has_no_gap_between_split_ranges() {
    let t = load(two_zone());
    assert_eq!(period_at(&t, sofia(2026, 2, 10, 23, 59)), "night");
    assert_eq!(period_at(&t, sofia(2026, 2, 11, 0, 0)), "night");
}

// ---------- weekend / holiday ----------

#[test]
fn weekend_is_all_night() {
    let t = load(two_zone());
    // Sat 2026-02-14 and Sun 2026-02-15, midday
    assert_eq!(period_at(&t, sofia(2026, 2, 14, 12, 0)), "night");
    assert_eq!(period_at(&t, sofia(2026, 2, 15, 12, 0)), "night");
    // Monday midday is day
    assert_eq!(period_at(&t, sofia(2026, 2, 16, 12, 0)), "day");
}

#[test]
fn holiday_on_weekday_uses_holiday_rule() {
    let t = load(two_zone());
    // Tue 2026-03-03 listed as holiday
    assert_eq!(period_at(&t, sofia(2026, 3, 3, 12, 0)), "night");
    // Wed 2026-03-04 is a normal day
    assert_eq!(period_at(&t, sofia(2026, 3, 4, 12, 0)), "day");
}

// ---------- seasons ----------

#[test]
fn season_switch_changes_night_hours() {
    let t = load(two_zone());
    // Tue 2026-03-31 (winter): 22:30 is night
    assert_eq!(period_at(&t, sofia(2026, 3, 31, 22, 30)), "night");
    assert_eq!(t.resolve(sofia(2026, 3, 31, 22, 30)).season_id, "winter");
    // Wed 2026-04-01 (summer): 22:30 is still day, 23:00 night
    assert_eq!(period_at(&t, sofia(2026, 4, 1, 22, 30)), "day");
    assert_eq!(period_at(&t, sofia(2026, 4, 1, 23, 0)), "night");
    // Fri 2026-10-30 (summer) 06:30 is night; Mon 2026-11-02 (winter) 06:30 is day
    assert_eq!(period_at(&t, sofia(2026, 10, 30, 6, 30)), "night");
    assert_eq!(period_at(&t, sofia(2026, 11, 2, 6, 30)), "day");
}

#[test]
fn season_wrapping_year_end_covers_january() {
    let t = load(two_zone());
    assert_eq!(t.resolve(sofia(2027, 1, 15, 12, 0)).season_id, "winter");
    assert_eq!(t.resolve(sofia(2027, 12, 15, 12, 0)).season_id, "winter");
}

// ---------- DST ----------

fn dst_tariff() -> Tariff {
    load(json!({
        "schema_version": 1, "id": "dst", "name": "dst", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": "2026-01-01",
        "components": [{"id": "e", "kind": "energy", "vat_inclusive": true,
                        "per_period": {"day": "0.2", "night": "0.1"}}],
        "periods": [{"id": "day", "label": "Day"}, {"id": "night", "label": "Night"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": ranges(&[("00:00","06:00"),("22:00","24:00")]), "period": "night"},
            {"days": ["all"], "ranges": ranges(&[("06:00","22:00")]), "period": "day"}
        ]}]}
    }))
}

/// Count minutes per period between two local midnights, walking real UTC minutes.
fn count_day(t: &Tariff, y: i32, m: u32, d: u32) -> (usize, usize, usize) {
    let start = sofia(y, m, d, 0, 0);
    let end = (NaiveDate::from_ymd_opt(y, m, d).unwrap().succ_opt().unwrap()).and_hms_opt(0, 0, 0).unwrap();
    let end = chrono_tz::Europe::Sofia.from_local_datetime(&end).single().unwrap().with_timezone(&Utc);
    let (mut total, mut night, mut day) = (0, 0, 0);
    let mut cur = start;
    while cur < end {
        match t.resolve(cur).period_id {
            "night" => night += 1,
            _ => day += 1,
        }
        total += 1;
        cur += chrono::Duration::minutes(1);
    }
    (total, night, day)
}

#[test]
fn dst_spring_forward_day_has_23_hours_and_loses_a_night_hour() {
    // Sofia 2026-03-29: 03:00 -> 04:00 (clocks forward). The skipped hour is inside 00:00-06:00 night.
    let (total, night, day) = count_day(&dst_tariff(), 2026, 3, 29);
    assert_eq!(total, 23 * 60);
    assert_eq!(night, (5 + 2) * 60);
    assert_eq!(day, 16 * 60);
}

#[test]
fn dst_fall_back_day_has_25_hours_and_gains_a_night_hour() {
    // Sofia 2026-10-25: 04:00 -> 03:00 (clocks back). The repeated hour is inside 00:00-06:00 night.
    let (total, night, day) = count_day(&dst_tariff(), 2026, 10, 25);
    assert_eq!(total, 25 * 60);
    assert_eq!(night, (7 + 2) * 60);
    assert_eq!(day, 16 * 60);
}

#[test]
fn ambiguous_local_time_resolves_for_both_utc_instants() {
    let t = dst_tariff();
    // 2026-10-25 03:30 local happens twice: 00:30 UTC (EEST) and 01:30 UTC (EET)
    let first = Utc.with_ymd_and_hms(2026, 10, 25, 0, 30, 0).unwrap();
    let second = Utc.with_ymd_and_hms(2026, 10, 25, 1, 30, 0).unwrap();
    assert_eq!(t.resolve(first).local, t.resolve(second).local);
    assert_eq!(period_at(&t, first), "night");
    assert_eq!(period_at(&t, second), "night");
}

// ---------- price composition ----------

#[test]
fn price_composition_handles_vat_inclusive_and_exclusive_components() {
    let mut v = two_zone();
    v["components"] = json!([
        {"id": "energy", "kind": "energy", "vat_inclusive": false, "per_kwh": "0.10"},
        {"id": "network", "kind": "network", "vat_inclusive": false, "per_kwh": "0.05"},
        {"id": "levy", "kind": "levy", "vat_inclusive": true, "per_kwh": "0.012"}
    ]);
    let t = load(v);
    let p = t.price("day").unwrap();
    // gross = 0.15 * 1.2 + 0.012 = 0.192 ; excl = 0.15 + 0.012/1.2 = 0.16 ; vat = 0.032
    assert_eq!(p.gross, dec("0.192"));
    assert_eq!(p.excl_vat, dec("0.16"));
    assert_eq!(p.vat, dec("0.032"));
}

#[test]
fn per_period_components_price_each_period_separately() {
    let t = load(two_zone());
    // day: (0.10 + 0.03) * 1.2 = 0.156 ; night: (0.05 + 0.03) * 1.2 = 0.096
    assert_eq!(t.price("day").unwrap().gross, dec("0.156"));
    assert_eq!(t.price("night").unwrap().gross, dec("0.096"));
    assert!(t.price("nope").is_none());
}

#[test]
fn fixed_fees_are_reported_separately_with_vat() {
    let t = load(two_zone());
    assert_eq!(t.monthly_fixed_gross(), dec("1.2"));
}

#[test]
fn flat_tariff_is_the_simple_case() {
    let t = load(json!({
        "schema_version": 1, "id": "flat", "name": "Flat", "country": "DE", "currency": "EUR",
        "timezone": "Europe/Berlin", "vat_percent": "19", "valid_from": "2026-01-01",
        "components": [{"id": "all_in", "kind": "energy", "vat_inclusive": true, "per_kwh": "0.30"}],
        "periods": [{"id": "flat", "label": "Flat"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": ranges(&[("00:00","24:00")]), "period": "flat"}
        ]}]}
    }));
    assert_eq!(period_at(&t, local("Europe/Berlin", 2026, 6, 1, 13, 0)), "flat");
    assert_eq!(t.price("flat").unwrap().gross, dec("0.30"));
}

// ---------- validation ----------

fn expect_invalid(v: Value, needle: &str) {
    match Tariff::from_value(v) {
        Err(TariffError::Invalid(m)) => assert!(m.contains(needle), "message '{m}' should contain '{needle}'"),
        other => panic!("expected Invalid containing '{needle}', got {other:?}"),
    }
}

#[test]
fn rejects_uncovered_minutes() {
    let mut v = two_zone();
    v["schedule"]["seasons"][0]["rules"][1]["ranges"] = ranges(&[("06:00", "21:00")]);
    expect_invalid(v, "not covered");
}

#[test]
fn rejects_overlapping_ranges() {
    let mut v = two_zone();
    v["schedule"]["seasons"][0]["rules"][1]["ranges"] = ranges(&[("05:00", "22:00")]);
    expect_invalid(v, "more than once");
}

#[test]
fn rejects_ranges_that_wrap_midnight() {
    let mut v = two_zone();
    v["schedule"]["seasons"][0]["rules"][0]["ranges"] = ranges(&[("22:00", "06:00")]);
    expect_invalid(v, "split ranges at midnight");
}

#[test]
fn rejects_unknown_period_and_timezone() {
    let mut v = two_zone();
    v["schedule"]["seasons"][0]["rules"][0]["period"] = json!("weekend_super");
    expect_invalid(v, "unknown period");

    let mut v = two_zone();
    v["timezone"] = json!("Mars/Olympus");
    expect_invalid(v, "unknown timezone");
}

#[test]
fn rejects_season_gaps_and_overlaps() {
    let mut v = two_zone();
    v["schedule"]["seasons"][1]["from"] = json!("04-02"); // 04-01 uncovered
    expect_invalid(v, "belongs to 0 seasons");

    let mut v = two_zone();
    v["schedule"]["seasons"][1]["from"] = json!("03-31"); // 03-31 twice
    expect_invalid(v, "belongs to 2 seasons");
}

#[test]
fn rejects_component_missing_a_period_price() {
    let mut v = two_zone();
    v["components"][0]["per_period"] = json!({"day": "0.10"});
    expect_invalid(v, "no price for period 'night'");
}

#[test]
fn schema_rejects_numbers_instead_of_decimal_strings() {
    let mut v = two_zone();
    v["vat_percent"] = json!(20); // must be a string: no floats in money data
    assert!(matches!(Tariff::from_value(v), Err(TariffError::Schema(_))));
}

#[test]
fn schema_rejects_unknown_fields_and_bad_times() {
    let mut v = two_zone();
    v["surprise"] = json!(true);
    assert!(matches!(Tariff::from_value(v), Err(TariffError::Schema(_))));

    let mut v = two_zone();
    v["schedule"]["seasons"][0]["rules"][0]["ranges"] = ranges(&[("25:00", "26:00")]);
    assert!(matches!(Tariff::from_value(v), Err(TariffError::Schema(_))));
}

#[test]
fn schema_rejects_component_with_both_price_forms() {
    let mut v = two_zone();
    v["components"][1]["per_period"] = json!({"day": "0.01", "night": "0.01"});
    assert!(matches!(Tariff::from_value(v), Err(TariffError::Schema(_))));
}

// ---------- staleness ----------

#[test]
fn warns_when_expired_unverified_or_not_yet_valid() {
    let t = load(two_zone());
    let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();

    assert!(t.warnings(d(2026, 11, 6)).is_empty());
    assert!(t.warnings(d(2027, 7, 1)).contains(&Warning::Expired { valid_to: d(2027, 6, 30) }));
    assert!(t.warnings(d(2026, 6, 1)).contains(&Warning::NotYetValid { valid_from: d(2026, 7, 1) }));
    assert!(t.warnings(d(2028, 1, 1)).iter().any(|w| matches!(w, Warning::VerifiedLongAgo { .. })));

    let mut v = two_zone();
    v["last_verified"] = Value::Null;
    v["verification"] = json!("unverified");
    let t = load(v);
    let w = t.warnings(d(2026, 11, 6));
    assert!(w.contains(&Warning::NeverVerified));
    assert!(w.contains(&Warning::MarkedUnverified));
}

// ---------- "prices usually change on ..." reminder ----------

fn with_update(valid_from: &str, last_verified: Option<&str>, typical: &str) -> Tariff {
    let mut v = two_zone();
    v["valid_from"] = json!(valid_from);
    v["valid_to"] = Value::Null;
    v["last_verified"] = last_verified.map_or(Value::Null, |s| json!(s));
    v["typical_update"] = json!(typical);
    load(v)
}

fn due(t: &Tariff, y: i32, m: u32, d: u32) -> Option<NaiveDate> {
    t.warnings(NaiveDate::from_ymd_opt(y, m, d).unwrap()).into_iter().find_map(|w| match w {
        Warning::UpdateLikelyDue { since } => Some(since),
        _ => None,
    })
}

#[test]
fn update_reminder_fires_after_the_usual_change_date_and_not_before() {
    let t = with_update("2026-07-01", Some("2026-11-01"), "07-01");
    assert_eq!(due(&t, 2026, 11, 6), None, "this version started on the change date itself");
    assert_eq!(due(&t, 2027, 6, 30), None);
    assert_eq!(due(&t, 2027, 7, 1), NaiveDate::from_ymd_opt(2027, 7, 1));
    assert_eq!(due(&t, 2027, 12, 31), NaiveDate::from_ymd_opt(2027, 7, 1));
}

#[test]
fn re_verifying_after_the_change_date_clears_the_reminder() {
    let t = with_update("2026-07-01", Some("2027-07-05"), "07-01");
    assert_eq!(due(&t, 2027, 7, 10), None);
    // ...but it comes back the following year
    assert_eq!(due(&t, 2028, 7, 2), NaiveDate::from_ymd_opt(2028, 7, 1));
}

#[test]
fn a_never_verified_older_tariff_is_reminded_too() {
    let t = with_update("2025-07-01", None, "07-01");
    assert_eq!(due(&t, 2026, 7, 2), NaiveDate::from_ymd_opt(2026, 7, 1));
}

#[test]
fn the_reminder_uses_the_previous_years_date_early_in_the_year() {
    let t = with_update("2026-07-01", Some("2026-11-01"), "01-15");
    assert_eq!(due(&t, 2027, 1, 10), None, "Jan 15 2026 is before this version started");
    assert_eq!(due(&t, 2027, 1, 16), NaiveDate::from_ymd_opt(2027, 1, 15));
}

#[test]
fn tariffs_without_the_field_never_remind_and_bad_dates_are_rejected() {
    let plain = load(two_zone());
    assert_eq!(due(&plain, 2030, 8, 1), None);
    let mut v = two_zone();
    v["typical_update"] = json!("02-30");
    assert!(matches!(Tariff::from_value(v), Err(TariffError::Invalid(m)) if m.contains("month-day")));
    let mut v = two_zone();
    v["typical_update"] = json!("13-01");
    assert!(matches!(Tariff::from_value(v), Err(TariffError::Schema(_))));
}