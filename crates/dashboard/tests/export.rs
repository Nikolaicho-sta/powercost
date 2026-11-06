//! Exports and the fixed-fee option. Tariffs are SYNTHETIC.

use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::TariffSet;
use dashboard::*;
use estimator::{Estimator, EstimatorConfig};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::str::FromStr;
use storage::Storage;
use tariff_engine::Tariff;

const SOFIA: Tz = chrono_tz::Europe::Sofia;

fn sofia(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    SOFIA.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().with_timezone(&Utc)
}

fn tariff(fixed_fee: Option<&str>) -> Tariff {
    let fees = match fixed_fee {
        Some(f) => json!([{"id": "meter", "per_month": f, "vat_inclusive": false}]),
        None => json!([]),
    };
    Tariff::from_value(json!({
        "schema_version": 1, "id": "t", "version": 1, "name": "Synthetic", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "20", "valid_from": "2026-01-01", "last_verified": "2026-10-01",
        "components": [{"id": "e", "kind": "energy", "vat_inclusive": false, "per_period": {"day": "0.20", "night": "0.10"}}],
        "fixed_fees": fees,
        "periods": [{"id": "day", "label": "Day"}, {"id": "night", "label": "Night"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": [{"start": "00:00", "end": "06:00"}, {"start": "22:00", "end": "24:00"}], "period": "night"},
            {"days": ["all"], "ranges": [{"start": "06:00", "end": "22:00"}], "period": "day"}]}]}
    }))
    .unwrap()
}

fn store_with(t: &Tariff, days: i64, until: DateTime<Utc>) -> Storage {
    let est = Estimator::new(EstimatorConfig::default()).unwrap();
    let spans = simulator::generate(&simulator::SimConfig::default(), &est, until - Duration::days(days), until, SOFIA);
    let mut s = Storage::open_in_memory().unwrap();
    s.record_spans(&spans, &TariffSet::single(t.clone())).unwrap();
    s
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

// ---------- fixed fees ----------

#[test]
fn fixed_fees_are_excluded_by_default_and_prorated_per_day_when_included() {
    let t = tariff(Some("3.00")); // 3.00 + 20% VAT = 3.60 / month
    let now = sofia(2026, 11, 30, 12, 0); // November has 30 days
    let s = store_with(&t, 40, now);

    let off = dashboard(&s, &t, PeriodKind::Month, now, None).unwrap();
    assert!(off.fixed_fees.is_none());
    assert_eq!(off.grand_total_minor, off.total.cost_minor);

    let on = dashboard_with(&s, &t, PeriodKind::Month, now, None, &DashOptions { include_fixed_fees: true }).unwrap();
    let fees = on.fixed_fees.unwrap();
    assert_eq!((fees.cost_nano, fees.cost_minor), (3_600_000_000, 360), "30 full days = one monthly fee");
    let expected = cost_engine::nano_to_minor(on.total.cost_nano + 3_600_000_000, 2);
    assert_eq!(on.grand_total_minor, expected);
    assert_eq!(on.total.cost_nano, off.total.cost_nano, "energy figures are not changed by the option");

    // today = one day's share: 3.60 / 30 = 0.12
    let today = dashboard_with(&s, &t, PeriodKind::Today, now, None, &DashOptions { include_fixed_fees: true }).unwrap();
    assert_eq!(today.fixed_fees.unwrap().cost_nano, 120_000_000);
}

#[test]
fn no_fixed_fee_means_no_line_even_when_included() {
    let t = tariff(None);
    let now = sofia(2026, 11, 10, 12, 0);
    let s = store_with(&t, 5, now);
    let d = dashboard_with(&s, &t, PeriodKind::Month, now, None, &DashOptions { include_fixed_fees: true }).unwrap();
    assert!(d.fixed_fees.is_none());
    assert_eq!(d.grand_total_minor, d.total.cost_minor);
}

#[test]
fn fixed_fee_proration_uses_the_real_length_of_each_month() {
    let t = tariff(Some("2.80")); // gross 3.36
    // February 2027: 28 days -> one day = 0.12 exactly; a leap February has 29
    assert_eq!(fixed_fees_nano(&t, sofia(2027, 2, 1, 0, 0), sofia(2027, 2, 2, 0, 0)), 120_000_000);
    let leap = fixed_fees_nano(&t, sofia(2028, 2, 1, 0, 0), sofia(2028, 2, 2, 0, 0));
    assert!(leap < 120_000_000 && leap > 115_000_000, "{leap}");
    // a range that spans two months mixes both lengths
    let span = fixed_fees_nano(&t, sofia(2026, 11, 30, 0, 0), sofia(2026, 12, 2, 0, 0));
    assert!(span > 0);
    assert_eq!(fixed_fees_nano(&t, sofia(2026, 11, 5, 0, 0), sofia(2026, 11, 5, 0, 0)), 0);
}

// ---------- export ----------

fn csv_rows(csv: &str) -> Vec<Vec<String>> {
    csv.lines().skip(1).map(|l| l.split(',').map(String::from).collect()).collect()
}

#[test]
fn csv_has_a_header_exact_decimals_and_matches_the_dashboard_totals() {
    let t = tariff(None);
    let now = sofia(2026, 11, 20, 12, 0);
    let s = store_with(&t, 20, now);
    let from = sofia(2026, 11, 1, 0, 0);
    let csv = export(&s, &t, from, now, Granularity::Day, ExportFormat::Csv, now).unwrap();
    assert!(csv.starts_with("local_start,utc_start,state,tariff_period,seconds,kwh,cost,currency\n"));
    let rows = csv_rows(&csv);
    assert!(rows.len() > 20);
    for r in &rows {
        assert_eq!(r.len(), 8, "{r:?}");
        assert_eq!(r[7], "EUR");
        assert!(r[5].split('.').nth(1).unwrap().len() == 6, "6 decimals: {}", r[5]);
    }
    let d = dashboard(&s, &t, PeriodKind::Month, now, None).unwrap();
    let kwh: Decimal = rows.iter().map(|r| dec(&r[5])).sum();
    let cost: Decimal = rows.iter().map(|r| dec(&r[6])).sum();
    let tol = Decimal::new(5, 7) * Decimal::from(rows.len() as i64);
    assert!((kwh - Decimal::from(d.total.kwh_nano) / Decimal::from(1_000_000_000)).abs() <= tol);
    assert!((cost - Decimal::from(d.total.cost_nano) / Decimal::from(1_000_000_000)).abs() <= tol);
}

#[test]
fn json_export_carries_the_disclaimer_tariff_and_the_same_rows() {
    let t = tariff(None);
    let now = sofia(2026, 11, 20, 12, 0);
    let s = store_with(&t, 10, now);
    let from = sofia(2026, 11, 10, 0, 0);
    let json_text = export(&s, &t, from, now, Granularity::Day, ExportFormat::Json, now).unwrap();
    let v: Value = serde_json::from_str(&json_text).unwrap();
    assert!(v["disclaimer"].as_str().unwrap().contains("ESTIMATE"));
    assert_eq!(v["tariff"]["id"], "t");
    assert_eq!(v["currency"], "EUR");
    assert_eq!(v["granularity"], "day");
    let csv_n = csv_rows(&export(&s, &t, from, now, Granularity::Day, ExportFormat::Csv, now).unwrap()).len();
    assert_eq!(v["rows"].as_array().unwrap().len(), csv_n);
    let shares = &v["estimate_method_share_permille"];
    let sum: i64 = ["measured_sensors", "calibrated_model", "default_model"].iter().map(|k| shares[*k].as_i64().unwrap()).sum();
    assert!((998..=1000).contains(&sum), "shares sum {sum}");
    // totals are exact decimal strings
    assert!(v["totals"]["cost"].as_str().unwrap().contains('.'));
}

#[test]
fn compaction_does_not_change_daily_or_hourly_exports() {
    let t = tariff(None);
    let now = sofia(2026, 11, 20, 12, 0);
    let mut s = store_with(&t, 30, now);
    let from = sofia(2026, 10, 25, 0, 0);
    let day_before = export(&s, &t, from, now, Granularity::Day, ExportFormat::Csv, now).unwrap();
    let hour_before = export(&s, &t, from, now, Granularity::Hour, ExportFormat::Csv, now).unwrap();
    s.compact(sofia(2026, 11, 10, 12, 0), SOFIA).unwrap();
    assert_eq!(export(&s, &t, from, now, Granularity::Day, ExportFormat::Csv, now).unwrap(), day_before);
    assert_eq!(export(&s, &t, from, now, Granularity::Hour, ExportFormat::Csv, now).unwrap(), hour_before);
}

#[test]
fn a_dst_day_exports_23_hourly_buckets() {
    let t = tariff(None);
    let s = store_with(&t, 3, sofia(2026, 3, 30, 0, 0));
    let csv = export(&s, &t, sofia(2026, 3, 29, 0, 0), sofia(2026, 3, 30, 0, 0), Granularity::Hour, ExportFormat::Csv, Utc::now()).unwrap();
    let hours: BTreeSet<String> = csv_rows(&csv).into_iter().map(|r| r[1].clone()).collect();
    assert_eq!(hours.len(), 23);
}

#[test]
fn empty_ranges_export_a_header_only_or_no_rows() {
    let t = tariff(None);
    let s = Storage::open_in_memory().unwrap();
    let now = sofia(2026, 11, 20, 12, 0);
    let csv = export(&s, &t, now - Duration::days(3), now, Granularity::Day, ExportFormat::Csv, now).unwrap();
    assert_eq!(csv.lines().count(), 1);
    let v: Value = serde_json::from_str(&export(&s, &t, now - Duration::days(3), now, Granularity::Day, ExportFormat::Json, now).unwrap()).unwrap();
    assert_eq!(v["rows"].as_array().unwrap().len(), 0);
    assert_eq!(v["totals"]["cost"], "0.000000");
}
