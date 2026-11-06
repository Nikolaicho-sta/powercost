#![allow(clippy::field_reassign_with_default)] // tests build settings step by step for readability
use estimator::Calibration;
use pc_core::PowerState;
use rust_decimal::Decimal;
use settings::*;
use std::collections::BTreeSet;
use std::str::FromStr;
use storage::Storage;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

// ---------- settings ----------

#[test]
fn defaults_are_valid_and_privacy_friendly() {
    let s = Settings::default();
    assert!(s.validate().is_ok());
    assert!(!s.region.ip_lookup_opt_in, "network lookups are opt-in");
    assert!(!s.include_fixed_fees, "fixed fees are excluded by default");
    assert!(!s.estimator.include_monitor, "monitor is off by default");
    assert!(!s.autostart && !s.first_run_done && s.budget.is_none());
}

#[test]
fn an_old_or_partial_document_loads_with_defaults_for_the_rest() {
    let s: Settings = serde_json::from_str(r#"{"first_run_done": true, "theme": "dark", "region": {"country": "BG"}}"#).unwrap();
    assert!(s.first_run_done);
    assert_eq!(s.theme, Theme::Dark);
    assert_eq!(s.region.country.as_deref(), Some("BG"));
    assert!(!s.region.confirmed);
    assert_eq!(s.detail_retention_days, 90);
    assert_eq!(s.estimator.psu_efficiency, d("0.85"));
    // unknown future fields are ignored rather than fatal
    assert!(serde_json::from_str::<Settings>(r#"{"from_the_future": 1}"#).is_ok());
}

#[test]
fn settings_roundtrip_through_the_database() {
    let store = Storage::open_in_memory().unwrap();
    assert_eq!(Settings::load(&store).unwrap(), Settings::default());
    let mut s = Settings::default();
    s.first_run_done = true;
    s.theme = Theme::Light;
    s.tariff_id = "user-home".into();
    s.budget = Some(Budget { monthly_minor: 2500, alert_percents: vec![50, 90, 100] });
    s.estimator.calibration = Some(Calibration::new(d("22"), d("95")).unwrap());
    s.estimator.include_monitor = true;
    s.estimator.unplugged_when_off = true;
    s.gap_assumptions.crash = PowerState::Idle;
    s.save(&store).unwrap();
    assert_eq!(Settings::load(&store).unwrap(), s);
}

#[test]
fn invalid_settings_report_every_problem_and_are_never_saved() {
    let store = Storage::open_in_memory().unwrap();
    let mut s = Settings::default();
    s.estimator.psu_efficiency = d("1.4");
    s.thresholds.cpu_enter_percent = 5;
    s.thresholds.cpu_exit_percent = 30;
    s.thresholds.input_idle_secs = 1;
    s.thresholds.exit_debounce_ticks = 0;
    s.detail_retention_days = 1;
    s.tariff_id = "  ".into();
    s.region.country = Some("bulgaria".into());
    s.budget = Some(Budget { monthly_minor: 0, alert_percents: vec![] });
    let errs = s.validate().unwrap_err();
    assert!(errs.len() >= 8, "{errs:?}");
    assert!(s.save(&store).is_err());
    assert_eq!(Settings::load(&store).unwrap(), Settings::default(), "nothing was written");
}

#[test]
fn gap_assumptions_map_to_the_classifier() {
    let a = GapAssumptions { crash: PowerState::Idle, unknown: PowerState::Sleep }.to_classifier();
    assert_eq!((a.crash, a.unknown), (PowerState::Idle, PowerState::Sleep));
}

// ---------- budget ----------

fn budget() -> Budget {
    Budget { monthly_minor: 1000, alert_percents: vec![80, 100] } // 10.00
}

#[test]
fn thresholds_fire_when_reached_in_ascending_order() {
    let none = BTreeSet::new();
    assert!(due_alerts(&budget(), 799, &none).is_empty());
    assert_eq!(due_alerts(&budget(), 800, &none), vec![80]);
    assert_eq!(due_alerts(&budget(), 999, &none), vec![80]);
    assert_eq!(due_alerts(&budget(), 1000, &none), vec![80, 100]);
    assert_eq!(due_alerts(&budget(), 5000, &none), vec![80, 100]);
}

#[test]
fn a_threshold_fires_once_per_month() {
    let store = Storage::open_in_memory().unwrap();
    let nov = month_key(2026, 11);
    let sent = load_sent(&store, &nov).unwrap();
    assert_eq!(due_alerts(&budget(), 850, &sent), vec![80]);
    mark_sent(&store, &nov, &[80]).unwrap();
    let sent = load_sent(&store, &nov).unwrap();
    assert!(due_alerts(&budget(), 850, &sent).is_empty(), "already notified");
    assert_eq!(due_alerts(&budget(), 1000, &sent), vec![100]);
    // a new month starts clean
    assert_eq!(due_alerts(&budget(), 850, &load_sent(&store, &month_key(2026, 12)).unwrap()), vec![80]);
    assert_eq!(month_key(2026, 3), "2026-03");
}

#[test]
fn unsorted_duplicate_percentages_and_bad_budgets_are_handled() {
    let b = Budget { monthly_minor: 1000, alert_percents: vec![100, 50, 50, 150] };
    assert_eq!(due_alerts(&b, 1200, &BTreeSet::new()), vec![50, 100]);
    assert!(due_alerts(&Budget { monthly_minor: 0, alert_percents: vec![100] }, 10_000, &BTreeSet::new()).is_empty());
    assert!(due_alerts(&budget(), i64::MAX, &BTreeSet::new()).len() == 2, "no overflow on huge amounts");
}

#[test]
fn alert_text_distinguishes_warning_from_exceeded() {
    let (t, b) = alert_text(80, "8.10 EUR", "10.00 EUR");
    assert!(t.contains("80%") && b.contains("8.10 EUR"));
    let (t, _) = alert_text(100, "10.20 EUR", "10.00 EUR");
    assert!(t.contains("reached"));
}
