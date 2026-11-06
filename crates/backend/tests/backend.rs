#![allow(clippy::field_reassign_with_default)] // tests build settings step by step for readability
use backend::*;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::TariffSet;
use estimator::{CalibrationPoint, Estimator, EstimatorConfig, Inputs, Layer};
use pc_core::PowerState;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use settings::{Budget, Settings};
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use storage::{Span, SpanSource, Storage};
use tariff_engine::Catalog;

const SOFIA: Tz = chrono_tz::Europe::Sofia;

fn sofia(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    SOFIA.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().with_timezone(&Utc)
}
fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}
fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn tariffs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tariffs")
}

fn catalog() -> Catalog {
    let mut c = Catalog::new();
    for f in ["bg-electrohold.json", "bg-evn.json", "bg-energo-pro.json"] {
        c.add_bundled(f, &fs::read_to_string(tariffs_dir().join(f)).unwrap());
    }
    c
}

fn user_tariff(version: u32, valid_from: &str, price: &str) -> Value {
    json!({
        "schema_version": 1, "id": "user-home", "version": version, "name": "Home", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": valid_from, "last_verified": "2026-11-01",
        "components": [{"id": "all", "kind": "energy", "vat_inclusive": true, "per_kwh": price}],
        "periods": [{"id": "flat", "label": "Flat"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": [{"start": "00:00", "end": "24:00"}], "period": "flat"}]}]}
    })
}

fn span(a: DateTime<Utc>, b: DateTime<Utc>, _state: PowerState, watts: &str, inputs: Inputs) -> Span {
    Span { start: a, end: b, inputs, watts: dec(watts), layer: Layer::Default, billable: true, source: SpanSource::Simulated }
}

fn near(a: i64, b: i64, tol: i64) {
    assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
}

// ---------- active tariff ----------

#[test]
fn active_tariff_uses_the_selected_id_and_all_its_versions() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = catalog();
    let mut s = Storage::open_in_memory().unwrap();
    c.save_user(dir.path(), user_tariff(1, "2025-01-01", "0.10")).unwrap();
    c.save_user(dir.path(), user_tariff(2, "2026-07-01", "0.12")).unwrap();
    let mut settings = Settings::default();
    settings.tariff_id = "user-home".into();
    let a = active_tariff(&c, &settings, day(2026, 11, 6)).unwrap();
    assert_eq!((a.current.id.as_str(), a.current.version), ("user-home", 2));
    assert!(a.fallback_note.is_none());
    // pricing history before the change uses v1, after uses v2
    let spans = [span(sofia(2026, 6, 30, 12, 0), sofia(2026, 6, 30, 13, 0), PowerState::Active, "1000", Inputs::new(PowerState::Active)), span(sofia(2026, 7, 1, 12, 0), sofia(2026, 7, 1, 13, 0), PowerState::Active, "1000", Inputs::new(PowerState::Active))];
    s.record_spans(&spans, &a.set).unwrap();
    let days = s.daily_totals(sofia(2026, 6, 30, 0, 0), sofia(2026, 7, 2, 0, 0), SOFIA).unwrap();
    near(days[&day(2026, 6, 30)].cost_nano, 100_000_000, 60);
    near(days[&day(2026, 7, 1)].cost_nano, 120_000_000, 60);
}

#[test]
fn a_missing_tariff_falls_back_with_a_note_and_an_empty_catalogue_is_an_error() {
    let c = catalog();
    let mut settings = Settings::default();
    settings.tariff_id = "deleted-one".into();
    let a = active_tariff(&c, &settings, day(2026, 11, 6)).unwrap();
    assert!(a.fallback_note.unwrap().contains("deleted-one"));
    assert!(active_tariff(&Catalog::new(), &settings, day(2026, 11, 6)).is_err());
}

// ---------- tariff editor ----------

#[test]
fn live_validation_previews_prices_and_reports_all_problems() {
    let electrohold: Value = serde_json::from_str(&fs::read_to_string(tariffs_dir().join("bg-electrohold.json")).unwrap()).unwrap();
    let r = validate_tariff(electrohold.clone(), day(2026, 11, 6));
    assert!(r.ok && r.errors.is_empty());
    let day_price = r.prices.iter().find(|p| p.period_id == "day").unwrap();
    assert_eq!((day_price.excl_vat.as_str(), day_price.gross.as_str()), ("0.12854", "0.154248"));

    let mut bad = electrohold;
    bad["vat_percent"] = json!(20); // a number, not a decimal string
    bad["schedule"]["seasons"][0]["rules"][0]["ranges"] = json!([{"start": "00:00", "end": "05:00"}]);
    let r = validate_tariff(bad, day(2026, 11, 6));
    assert!(!r.ok && !r.errors.is_empty() && r.prices.is_empty());

    let evn: Value = serde_json::from_str(&fs::read_to_string(tariffs_dir().join("bg-evn.json")).unwrap()).unwrap();
    let r = validate_tariff(evn, day(2026, 11, 6));
    assert!(r.ok && r.warnings.iter().any(|w| w.code == "unverified"), "unverified presets keep their warning");
}

#[test]
fn saving_the_active_tariff_reprices_only_the_days_from_valid_from() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = catalog();
    let mut s = Storage::open_in_memory().unwrap();
    c.save_user(dir.path(), user_tariff(1, "2025-01-01", "0.10")).unwrap();
    let set = TariffSet::new(c.versions("user-home")).unwrap();
    // 1 kWh a day for 6 days (Nov 1..6) at 0.10
    let spans: Vec<Span> = (1..=6).map(|d| span(sofia(2026, 11, d, 12, 0), sofia(2026, 11, d, 13, 0), PowerState::Active, "1000", Inputs::new(PowerState::Active))).collect();
    s.record_spans(&spans, &set).unwrap();
    let now = sofia(2026, 11, 6, 18, 0);
    let before = s.daily_totals(sofia(2026, 11, 1, 0, 0), now, SOFIA).unwrap();

    // price rises to 0.30 from Nov 4
    let res = save_tariff(&mut s, &mut c, dir.path(), user_tariff(2, "2026-11-04", "0.30"), "user-home", None, now).unwrap();
    assert_eq!((res.info.id.as_str(), res.info.version), ("user-home", 2));
    assert!(res.recomputed_spans.unwrap() >= 3);
    assert!(dir.path().join("user-home.v2.json").exists());
    let after = s.daily_totals(sofia(2026, 11, 1, 0, 0), now, SOFIA).unwrap();
    for d in 1..=3 {
        assert_eq!(before[&day(2026, 11, d)], after[&day(2026, 11, d)], "Nov {d} is before valid_from and must not change");
    }
    for d in 4..=6 {
        near(after[&day(2026, 11, d)].cost_nano, 300_000_000, 60);
    }
}

#[test]
fn saving_a_non_active_tariff_does_not_touch_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = catalog();
    let mut s = Storage::open_in_memory().unwrap();
    let t = TariffSet::new(c.versions("bg-electrohold")).unwrap();
    s.record_spans(&[span(sofia(2026, 11, 2, 12, 0), sofia(2026, 11, 2, 13, 0), PowerState::Active, "1000", Inputs::new(PowerState::Active))], &t).unwrap();
    let before = s.summary(sofia(2026, 11, 1, 0, 0), sofia(2026, 11, 7, 0, 0)).unwrap().total;
    let res = save_tariff(&mut s, &mut c, dir.path(), user_tariff(1, "2026-01-01", "0.99"), "bg-electrohold", None, sofia(2026, 11, 6, 0, 0)).unwrap();
    assert!(res.recomputed_spans.is_none());
    assert_eq!(s.summary(sofia(2026, 11, 1, 0, 0), sofia(2026, 11, 7, 0, 0)).unwrap().total, before);
}

#[test]
fn a_correction_older_than_the_kept_detail_reports_the_clipping() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = catalog();
    let mut s = Storage::open_in_memory().unwrap();
    c.save_user(dir.path(), user_tariff(1, "2025-01-01", "0.10")).unwrap();
    let set = TariffSet::new(c.versions("user-home")).unwrap();
    let spans: Vec<Span> = (1..=20).map(|d| span(sofia(2026, 10, d, 12, 0), sofia(2026, 10, d, 13, 0), PowerState::Active, "1000", Inputs::new(PowerState::Active))).collect();
    s.record_spans(&spans, &set).unwrap();
    s.compact(sofia(2026, 10, 15, 12, 0), SOFIA).unwrap();
    let res = save_tariff(&mut s, &mut c, dir.path(), user_tariff(2, "2026-10-01", "0.50"), "user-home", None, sofia(2026, 10, 25, 0, 0)).unwrap();
    assert!(res.clipped_by_compaction);
}

#[test]
fn an_invalid_tariff_is_rejected_and_nothing_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = catalog();
    let mut s = Storage::open_in_memory().unwrap();
    let mut bad = user_tariff(1, "2026-01-01", "0.10");
    bad["periods"] = json!([]);
    assert!(save_tariff(&mut s, &mut c, dir.path(), bad, "user-home", None, Utc::now()).is_err());
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
}

// ---------- calibration ----------

fn calibration_points(idle: &str, full: &str) -> Vec<CalibrationPoint> {
    vec![CalibrationPoint { util_permille: 0, watts: dec(idle) }, CalibrationPoint { util_permille: 1000, watts: dec(full) }]
}

fn default_span(s: &mut Storage, set: &TariffSet, a: DateTime<Utc>, b: DateTime<Utc>) {
    let inputs = Inputs { cpu_load_pct: 50.0, ..Inputs::new(PowerState::Idle) };
    let est = Estimator::new(EstimatorConfig::default()).unwrap().estimate(&inputs);
    s.record_span(&Span { start: a, end: b, inputs, watts: est.watts, layer: est.layer, billable: true, source: SpanSource::Simulated }, set).unwrap();
}

#[test]
fn calibration_is_saved_reestimates_history_and_can_be_cleared() {
    let mut s = Storage::open_in_memory().unwrap();
    let c = catalog();
    let mut settings = Settings::default();
    let active = active_tariff(&c, &settings, day(2026, 11, 6)).unwrap();
    let (a, b) = (sofia(2026, 11, 5, 10, 0), sofia(2026, 11, 5, 11, 0));
    default_span(&mut s, &active.set, a, b);
    let now = sofia(2026, 11, 6, 12, 0);
    assert_eq!(s.spans_between(a, b).unwrap()[0].layer, Layer::Default);

    // idle 40 W, full 160 W -> 50% load = 100 W
    let r = set_calibration(&mut s, &mut settings, Some(calibration_points("40", "160")), 30, &active.set, now).unwrap();
    assert_eq!(r.spans, 1);
    let sp = &s.spans_between(a, b).unwrap()[0];
    assert_eq!((sp.watts, sp.layer), (dec("100.00"), Layer::Calibrated));
    assert!(Settings::load(&s).unwrap().estimator.calibration.is_some(), "persisted");
    near(s.summary(a, b).unwrap().total.kwh_nano, 100_000_000, 60);

    // clearing returns to the default model
    set_calibration(&mut s, &mut settings, None, 30, &active.set, now).unwrap();
    assert_eq!(s.spans_between(a, b).unwrap()[0].layer, Layer::Default);
    assert!(Settings::load(&s).unwrap().estimator.calibration.is_none());
}

#[test]
fn an_invalid_calibration_changes_nothing() {
    let mut s = Storage::open_in_memory().unwrap();
    let c = catalog();
    let mut settings = Settings::default();
    let active = active_tariff(&c, &settings, day(2026, 11, 6)).unwrap();
    let err = set_calibration(&mut s, &mut settings, Some(calibration_points("120", "100")), 30, &active.set, Utc::now());
    assert!(err.is_err());
    assert!(settings.estimator.calibration.is_none());
    assert!(Settings::load(&s).unwrap().estimator.calibration.is_none());
}

// ---------- budget ----------

fn heavy_hours(s: &mut Storage, set: &TariffSet, from_day: u32, hours: i64) {
    // 1 kW at 12:00 local on `from_day`, in the Electrohold day period: ~0.154 EUR per hour
    let a = sofia(2026, 11, from_day, 10, 0);
    s.record_span(&span(a, a + Duration::hours(hours), PowerState::Active, "1000", Inputs::new(PowerState::Active)), set).unwrap();
}

#[test]
fn budget_alerts_fire_once_per_threshold_and_month_with_the_highest_one() {
    let mut s = Storage::open_in_memory().unwrap();
    let c = catalog();
    let mut settings = Settings::default();
    settings.budget = Some(Budget { monthly_minor: 100, alert_percents: vec![50, 100] }); // 1.00 EUR
    let active = active_tariff(&c, &settings, day(2026, 11, 6)).unwrap();
    let now = sofia(2026, 11, 20, 12, 0);

    assert!(check_budget(&s, &settings, &active.current, now).unwrap().is_none(), "nothing spent yet");
    heavy_hours(&mut s, &active.set, 2, 4); // ~0.62 EUR = 62%
    let a = check_budget(&s, &settings, &active.current, now).unwrap().unwrap();
    assert_eq!(a.percent, 50);
    assert!(a.body.contains("EUR") && a.body.contains("1.00"));
    assert!(check_budget(&s, &settings, &active.current, now).unwrap().is_none(), "already notified");

    heavy_hours(&mut s, &active.set, 3, 4); // ~1.23 EUR
    let a = check_budget(&s, &settings, &active.current, now).unwrap().unwrap();
    assert_eq!(a.percent, 100);
    assert!(a.title.contains("reached"));
    assert!(check_budget(&s, &settings, &active.current, now).unwrap().is_none());

    // a new month starts clean (no spending yet in December -> no alert)
    assert!(check_budget(&s, &settings, &active.current, sofia(2026, 12, 3, 12, 0)).unwrap().is_none());
}

#[test]
fn a_big_jump_gives_one_alert_for_the_highest_threshold_and_no_budget_means_none() {
    let mut s = Storage::open_in_memory().unwrap();
    let c = catalog();
    let mut settings = Settings::default();
    let active = active_tariff(&c, &settings, day(2026, 11, 6)).unwrap();
    heavy_hours(&mut s, &active.set, 2, 12);
    let now = sofia(2026, 11, 20, 12, 0);
    assert!(check_budget(&s, &settings, &active.current, now).unwrap().is_none(), "no budget set");
    settings.budget = Some(Budget { monthly_minor: 100, alert_percents: vec![50, 100] });
    assert_eq!(check_budget(&s, &settings, &active.current, now).unwrap().unwrap().percent, 100);
    assert!(check_budget(&s, &settings, &active.current, now).unwrap().is_none(), "50% was marked as sent too");
}

#[test]
fn fixed_fees_count_toward_the_budget_only_when_included() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = catalog();
    let mut t = user_tariff(1, "2026-01-01", "0.10");
    t["fixed_fees"] = json!([{"id": "fee", "per_month": "30.00", "vat_inclusive": true}]);
    c.save_user(dir.path(), t).unwrap();
    let mut settings = Settings::default();
    settings.tariff_id = "user-home".into();
    settings.budget = Some(Budget { monthly_minor: 1000, alert_percents: vec![100] }); // 10 EUR
    let active = active_tariff(&c, &settings, day(2026, 11, 6)).unwrap();
    let s = Storage::open_in_memory().unwrap();
    let now = sofia(2026, 11, 20, 12, 0); // 20 days of a 30-day month: fee share = 20.00
    assert!(check_budget(&s, &settings, &active.current, now).unwrap().is_none(), "fees excluded by default");
    settings.include_fixed_fees = true;
    assert_eq!(check_budget(&s, &settings, &active.current, now).unwrap().unwrap().percent, 100);
}

// ---------- compaction ----------

#[test]
fn compaction_follows_the_retention_setting() {
    let mut s = Storage::open_in_memory().unwrap();
    let c = catalog();
    let mut settings = Settings::default();
    settings.detail_retention_days = 10;
    let active = active_tariff(&c, &settings, day(2026, 11, 6)).unwrap();
    for d in 1..=20u32 {
        default_span(&mut s, &active.set, sofia(2026, 10, d, 12, 0), sofia(2026, 10, d, 13, 0));
    }
    let now = sofia(2026, 10, 21, 12, 0);
    let rep = compact_if_due(&mut s, &settings, SOFIA, now).unwrap();
    assert_eq!(rep.compacted_until, Some(sofia(2026, 10, 11, 0, 0)));
    assert!(rep.minute_rows_removed > 0);
    assert_eq!(compact_if_due(&mut s, &settings, SOFIA, now).unwrap(), Default::default(), "idempotent");
}
