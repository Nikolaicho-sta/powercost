//! Browsing earlier periods (yesterday, last week, ...). Tariff is SYNTHETIC: flat 0.20, VAT 0.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::TariffSet;
use dashboard::*;
use estimator::{Inputs, Layer};
use pc_core::PowerState;
use rust_decimal::Decimal;
use serde_json::json;
use storage::{Span, SpanSource, Storage};
use tariff_engine::Tariff;

const SOFIA: Tz = chrono_tz::Europe::Sofia;

fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    SOFIA.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().with_timezone(&Utc)
}
fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn tariff() -> Tariff {
    Tariff::from_value(json!({
        "schema_version": 1, "id": "flat", "name": "Flat", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": "2026-01-01", "last_verified": "2026-10-01",
        "components": [{"id": "e", "kind": "energy", "vat_inclusive": true, "per_kwh": "0.20"}],
        "periods": [{"id": "flat", "label": "Flat"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": [{"start": "00:00", "end": "24:00"}], "period": "flat"}]}]}
    }))
    .unwrap()
}

/// One hour at 12:00 local on each day, drawing `n * 100` W on the n-th day, so every day differs.
fn store_with_days(first: NaiveDate, count: u32) -> Storage {
    let t = tariff();
    let set = TariffSet::single(t);
    let mut s = Storage::open_in_memory().unwrap();
    let mut spans = Vec::new();
    for n in 0..count {
        let d = first + Duration::days(n as i64);
        let (a, b) = (at(d.year_(), d.month0_() + 1, d.day_(), 12, 0), at(d.year_(), d.month0_() + 1, d.day_(), 13, 0));
        spans.push(Span {
            start: a,
            end: b,
            inputs: Inputs::new(PowerState::Active),
            watts: Decimal::from((n + 1) * 100),
            layer: Layer::Sensors,
            billable: true,
            source: SpanSource::Simulated,
        });
    }
    s.record_spans(&spans, &set).unwrap();
    s
}

// small helpers so the test reads naturally without importing Datelike everywhere
trait D {
    fn year_(&self) -> i32;
    fn month0_(&self) -> u32;
    fn day_(&self) -> u32;
}
impl D for NaiveDate {
    fn year_(&self) -> i32 {
        chrono::Datelike::year(self)
    }
    fn month0_(&self) -> u32 {
        chrono::Datelike::month0(self)
    }
    fn day_(&self) -> u32 {
        chrono::Datelike::day(self)
    }
}

fn near(a: i64, b: i64, tol: i64) {
    assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
}

// ---------- ranges ----------

#[test]
fn yesterday_is_a_complete_local_day_and_offset_zero_is_unchanged() {
    let now = at(2026, 11, 6, 15, 0);
    let y = range_for_offset(PeriodKind::Today, now, SOFIA, -1);
    assert_eq!((y.from, y.period_end, y.to), (at(2026, 11, 5, 0, 0), at(2026, 11, 6, 0, 0), at(2026, 11, 6, 0, 0)));
    assert_eq!(range_for_offset(PeriodKind::Today, now, SOFIA, 0), range_for(PeriodKind::Today, now, SOFIA));
    // a positive offset means "the future" and is not shifted
    assert_eq!(range_for_offset(PeriodKind::Today, now, SOFIA, 3), range_for(PeriodKind::Today, now, SOFIA));
}

#[test]
fn stepping_back_over_a_clock_change_gives_the_real_day_length() {
    let r = range_for_offset(PeriodKind::Today, at(2026, 3, 30, 12, 0), SOFIA, -1); // Sofia 2026-03-29: 23 h
    assert_eq!(r.period_end - r.from, Duration::hours(23));
    let r = range_for_offset(PeriodKind::Today, at(2026, 10, 26, 12, 0), SOFIA, -1); // 2026-10-25: 25 h
    assert_eq!(r.period_end - r.from, Duration::hours(25));
}

#[test]
fn previous_weeks_start_on_monday() {
    let now = at(2026, 11, 4, 15, 0); // Wednesday
    let w1 = range_for_offset(PeriodKind::Week, now, SOFIA, -1);
    assert_eq!((w1.from, w1.period_end), (at(2026, 10, 26, 0, 0), at(2026, 11, 2, 0, 0)));
    let w2 = range_for_offset(PeriodKind::Week, now, SOFIA, -2);
    assert_eq!(w2.from, at(2026, 10, 19, 0, 0));
}

#[test]
fn previous_months_and_years_cross_year_boundaries_without_overflow() {
    let jan = at(2027, 1, 15, 9, 0);
    let m1 = range_for_offset(PeriodKind::Month, jan, SOFIA, -1);
    assert_eq!((m1.from, m1.period_end), (at(2026, 12, 1, 0, 0), at(2027, 1, 1, 0, 0)));
    assert_eq!(range_for_offset(PeriodKind::Month, jan, SOFIA, -2).from, at(2026, 11, 1, 0, 0));
    assert_eq!(range_for_offset(PeriodKind::Month, jan, SOFIA, -14).from, at(2025, 11, 1, 0, 0));
    // the 31st must not break "one month back" (February has no 31st)
    let mar31 = at(2027, 3, 31, 12, 0);
    assert_eq!(range_for_offset(PeriodKind::Month, mar31, SOFIA, -1).from, at(2027, 2, 1, 0, 0));
    let y1 = range_for_offset(PeriodKind::Year, jan, SOFIA, -1);
    assert_eq!((y1.from, y1.period_end), (at(2026, 1, 1, 0, 0), at(2027, 1, 1, 0, 0)));
}

// ---------- dashboards for earlier periods ----------

#[test]
fn yesterday_shows_exactly_yesterdays_usage_and_a_complete_timeline() {
    let first = day(2026, 11, 1); // days 1..6: 100 W .. 600 W for one hour
    let s = store_with_days(first, 6);
    let t = tariff();
    let now = at(2026, 11, 6, 15, 0);
    let live = Some(CurrentInfo::new(PowerState::Active, &estimator::Estimator::new(estimator::EstimatorConfig::default()).unwrap().estimate(&Inputs::new(PowerState::Active)), Some(true)));

    let y = dashboard_at(&s, &t, PeriodKind::Today, now, -1, live.clone(), &DashOptions::default()).unwrap();
    // Nov 5 is the 5th day: 500 W for 1 h = 0.5 kWh = 0.10 EUR
    near(y.total.kwh_nano, 500_000_000, 60);
    near(y.total.cost_nano, 100_000_000, 60);
    assert_eq!((y.offset, y.period_start, y.period_end_inclusive), (-1, day(2026, 11, 5), day(2026, 11, 5)));
    assert_eq!(y.timeline.len(), 24);
    assert!(y.timeline.iter().all(|p| !p.future), "a past day has no upcoming hours");
    assert!(y.current.is_none(), "the live reading belongs to the current period only");
    assert!(y.has_earlier);

    let today = dashboard_at(&s, &t, PeriodKind::Today, now, 0, live, &DashOptions::default()).unwrap();
    near(today.total.kwh_nano, 600_000_000, 60);
    assert!(today.current.is_some());
    assert_eq!(today.timeline.iter().filter(|p| p.future).count(), 9); // 15:00 -> midnight
}

#[test]
fn walking_back_day_by_day_returns_each_days_own_figures() {
    let s = store_with_days(day(2026, 11, 1), 6);
    let t = tariff();
    let now = at(2026, 11, 6, 15, 0);
    for back in 0..6 {
        let d = dashboard_at(&s, &t, PeriodKind::Today, now, -back, None, &DashOptions::default()).unwrap();
        let expected = (6 - back as i64) * 100_000_000; // 600 W -> 0.6 kWh ... 100 W -> 0.1 kWh
        near(d.total.kwh_nano, expected, 60);
    }
    // before the first recorded day: empty, and nothing earlier to offer
    let before = dashboard_at(&s, &t, PeriodKind::Today, now, -6, None, &DashOptions::default()).unwrap();
    assert_eq!(before.total.kwh_nano, 0);
    assert!(!before.has_earlier);
}

#[test]
fn last_weeks_total_is_the_sum_of_its_days() {
    let s = store_with_days(day(2026, 10, 19), 21); // three weeks of data
    let t = tariff();
    let now = at(2026, 11, 4, 15, 0); // Wednesday of the 3rd week
    let week = dashboard_at(&s, &t, PeriodKind::Week, now, -1, None, &DashOptions::default()).unwrap();
    assert_eq!((week.period_start, week.period_end_inclusive), (day(2026, 10, 26), day(2026, 11, 1)));
    let mut days_sum = 0;
    for d in 0..7 {
        // 2026-10-26 + d: dashboards of those days, found by stepping back from `now` (Nov 4)
        let back = (day(2026, 11, 4) - (day(2026, 10, 26) + Duration::days(d))).num_days() as i32;
        days_sum += dashboard_at(&s, &t, PeriodKind::Today, now, -back, None, &DashOptions::default()).unwrap().total.kwh_nano;
    }
    assert_eq!(week.total.kwh_nano, days_sum);
    assert_eq!(week.timeline.len(), 7);
}

#[test]
fn earlier_months_and_years_are_complete_and_not_marked_upcoming() {
    let s = store_with_days(day(2026, 10, 1), 40);
    let t = tariff();
    let now = at(2026, 11, 20, 12, 0);
    let oct = dashboard_at(&s, &t, PeriodKind::Month, now, -1, None, &DashOptions::default()).unwrap();
    assert_eq!((oct.period_start, oct.period_end_inclusive), (day(2026, 10, 1), day(2026, 10, 31)));
    assert_eq!(oct.timeline.len(), 31);
    assert!(oct.timeline.iter().all(|p| !p.future));
    assert!(!oct.has_earlier, "the data starts on 1 October");
    let last_year = dashboard_at(&s, &t, PeriodKind::Year, now, -1, None, &DashOptions::default()).unwrap();
    assert_eq!(last_year.total.kwh_nano, 0);
    assert_eq!(last_year.timeline.len(), 12);
}

#[test]
fn compaction_does_not_change_an_earlier_days_figures() {
    let mut s = store_with_days(day(2026, 11, 1), 6);
    let t = tariff();
    let now = at(2026, 11, 6, 15, 0);
    let before: Vec<_> = (1..5).map(|b| dashboard_at(&s, &t, PeriodKind::Today, now, -b, None, &DashOptions::default()).unwrap()).collect();
    s.compact(at(2026, 11, 4, 12, 0), SOFIA).unwrap();
    for (i, b) in before.iter().enumerate() {
        let a = dashboard_at(&s, &t, PeriodKind::Today, now, -(i as i32 + 1), None, &DashOptions::default()).unwrap();
        assert_eq!(a.total.kwh_nano, b.total.kwh_nano, "day {} back", i + 1);
        assert_eq!(a.total.cost_nano, b.total.cost_nano);
        assert_eq!(a.timeline.iter().map(|p| p.total.kwh_nano).sum::<i64>(), b.timeline.iter().map(|p| p.total.kwh_nano).sum::<i64>());
    }
}

#[test]
fn an_empty_database_has_nothing_earlier() {
    let s = Storage::open_in_memory().unwrap();
    assert_eq!(s.earliest_data().unwrap(), None);
    let d = dashboard_at(&s, &tariff(), PeriodKind::Today, at(2026, 11, 6, 15, 0), -1, None, &DashOptions::default()).unwrap();
    assert!(!d.has_earlier);
    assert_eq!(d.total.kwh_nano, 0);
}

#[test]
fn the_earliest_data_survives_compaction() {
    let mut s = store_with_days(day(2026, 11, 1), 6);
    let first = s.earliest_data().unwrap().unwrap();
    assert_eq!(first, at(2026, 11, 1, 12, 0));
    s.compact(at(2026, 11, 5, 12, 0), SOFIA).unwrap();
    assert!(s.earliest_data().unwrap().is_some(), "aggregates still count as data");
    assert!(s.earliest_data().unwrap().unwrap() <= at(2026, 11, 2, 0, 0));
}
