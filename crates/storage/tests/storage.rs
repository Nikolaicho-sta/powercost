//! Storage tests. Tariffs are SYNTHETIC; the standby gap uses the real sanitised fixture.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::TariffSet;
use estimator::{Calibration, Estimator, EstimatorConfig, Inputs, Layer};
use gap_classifier::heartbeat::{Heartbeat, PowerSource};
use gap_classifier::*;
use pc_core::PowerState;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::str::FromStr;
use storage::*;
use tariff_engine::Tariff;

const SOFIA: Tz = chrono_tz::Europe::Sofia;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn sofia(y: i32, mo: u32, da: u32, h: u32, mi: u32) -> DateTime<Utc> {
    SOFIA.with_ymd_and_hms(y, mo, da, h, mi, 0).single().unwrap().with_timezone(&Utc)
}

fn ranges(v: &[(&str, &str)]) -> Value {
    Value::Array(v.iter().map(|(s, e)| json!({"start": s, "end": e})).collect())
}

/// VAT 0; night 00-06 + 22-24 at `night`, day at `day`.
fn tariff(version: u32, day: &str, night: &str) -> TariffSet {
    TariffSet::single(
        Tariff::from_value(json!({
            "schema_version": 1, "id": "t", "version": version, "name": "t", "country": "BG", "currency": "EUR",
            "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": "2026-01-01",
            "components": [{"id": "e", "kind": "energy", "vat_inclusive": true, "per_period": {"day": day, "night": night}}],
            "periods": [{"id": "day", "label": "Day"}, {"id": "night", "label": "Night"}],
            "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
                {"days": ["all"], "ranges": ranges(&[("00:00","06:00"),("22:00","24:00")]), "period": "night"},
                {"days": ["all"], "ranges": ranges(&[("06:00","22:00")]), "period": "day"}]}]}
        }))
        .unwrap(),
    )
}

fn span(start: DateTime<Utc>, end: DateTime<Utc>, state: PowerState, watts: &str) -> Span {
    Span {
        start,
        end,
        inputs: Inputs::new(state),
        watts: d(watts),
        layer: Layer::Default,
        billable: true,
        source: SpanSource::Simulated,
    }
}

fn near(a: i64, b: i64, tol: i64) {
    assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
}

// ---------- heartbeat / settings / events ----------

#[test]
fn heartbeat_roundtrip_and_prune() {
    let s = Storage::open_in_memory().unwrap();
    assert!(s.last_heartbeat().unwrap().is_none());
    let hb = |t: DateTime<Utc>, src| Heartbeat { time: t, session_id: "abc".into(), state: PowerState::Idle, source: src };
    s.write_heartbeat(&hb(sofia(2026, 5, 1, 10, 0), PowerSource::Ac)).unwrap();
    s.write_heartbeat(&hb(sofia(2026, 5, 1, 10, 1), PowerSource::Battery)).unwrap();
    let last = s.last_heartbeat().unwrap().unwrap();
    assert_eq!(last.time, sofia(2026, 5, 1, 10, 1));
    assert_eq!(last.source, PowerSource::Battery);
    assert_eq!(s.prune_heartbeats(sofia(2026, 5, 1, 10, 1)).unwrap(), 1);
    assert!(s.last_heartbeat().unwrap().is_some());
}

#[test]
fn settings_roundtrip_including_estimator_config() {
    let s = Storage::open_in_memory().unwrap();
    assert!(s.get_setting::<EstimatorConfig>("estimator").unwrap().is_none());
    let cfg = EstimatorConfig { calibration: Some(Calibration::new(d("20"), d("100")).unwrap()), ..EstimatorConfig::default() };
    s.set_setting("estimator", &cfg).unwrap();
    assert_eq!(s.get_setting::<EstimatorConfig>("estimator").unwrap().unwrap(), cfg);
    s.set_setting("estimator", &EstimatorConfig::default()).unwrap(); // overwrite
    assert!(s.get_setting::<EstimatorConfig>("estimator").unwrap().unwrap().calibration.is_none());
}

#[test]
fn power_events_are_persisted_deduplicated_and_queryable() {
    let mut s = Storage::open_in_memory().unwrap();
    let t0 = sofia(2026, 5, 1, 10, 0);
    let evs = vec![
        PowerEvent::new(t0, "Microsoft-Windows-Kernel-Power", 506).with("Reason", "6"),
        PowerEvent::new(t0 + Duration::hours(1), "Microsoft-Windows-Kernel-Power", 507).with("DurationInUs", "3600000000"),
    ];
    assert_eq!(s.save_power_events(&evs).unwrap(), 2);
    assert_eq!(s.save_power_events(&evs).unwrap(), 0, "re-saving the same events adds nothing");
    let back = s.power_events_between(t0, t0 + Duration::hours(2)).unwrap();
    assert_eq!(back, evs);
    assert_eq!(s.power_events_between(t0 + Duration::minutes(1), t0 + Duration::hours(2)).unwrap().len(), 1);
}

#[test]
fn data_survives_reopening_a_file_database_and_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pc.db");
    {
        let mut s = Storage::open(&path).unwrap();
        s.record_span(&span(sofia(2026, 5, 4, 10, 0), sofia(2026, 5, 4, 11, 0), PowerState::Idle, "100"), &tariff(1, "0.2", "0.1"))
            .unwrap();
    }
    let s = Storage::open(&path).unwrap();
    let sum = s.summary(sofia(2026, 5, 4, 0, 0), sofia(2026, 5, 5, 0, 0)).unwrap();
    near(sum.total.kwh_nano, 100_000_000, 60);
    drop(s);
    rusqlite_set_version(&path, 99);
    assert!(matches!(Storage::open(&path), Err(StorageError::TooNew(99))));
}

fn rusqlite_set_version(path: &std::path::Path, v: i64) {
    // Storage re-exports nothing for raw SQL, so use the sqlite3 file header: user_version is the
    // big-endian u32 at byte offset 60. WAL has been checkpointed on close.
    let mut bytes = std::fs::read(path).unwrap();
    bytes[60..64].copy_from_slice(&(v as u32).to_be_bytes());
    std::fs::write(path, bytes).unwrap();
}

// ---------- spans, costs, summary ----------

#[test]
fn recorded_span_is_priced_per_minute_and_summarised_by_state_and_period() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    // 21:30-22:30 local: 30 min day, 30 min night; 100 W Active
    s.record_span(&span(sofia(2026, 5, 4, 21, 30), sofia(2026, 5, 4, 22, 30), PowerState::Active, "100"), &t).unwrap();
    let sum = s.summary(sofia(2026, 5, 4, 0, 0), sofia(2026, 5, 5, 0, 0)).unwrap();
    near(sum.total.kwh_nano, 100_000_000, 60);
    near(sum.by_period["day"].cost_nano, 10_000_000, 30); // 0.05 kWh * 0.20
    near(sum.by_period["night"].cost_nano, 5_000_000, 30);
    assert_eq!(sum.by_state[&PowerState::Active].seconds, 3600);
    // a range that excludes the span is empty
    assert_eq!(s.summary(sofia(2026, 5, 5, 0, 0), sofia(2026, 5, 6, 0, 0)).unwrap().total.seconds, 0);
}

#[test]
fn back_to_back_spans_sharing_a_minute_add_up() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    let a = sofia(2026, 5, 4, 10, 0);
    s.record_span(&span(a, a + Duration::seconds(90), PowerState::Idle, "60"), &t).unwrap();
    s.record_span(&span(a + Duration::seconds(90), a + Duration::seconds(180), PowerState::Idle, "60"), &t).unwrap();
    let sum = s.summary(a, a + Duration::hours(1)).unwrap();
    assert_eq!(sum.total.seconds, 180);
    near(sum.total.kwh_nano, 3_000_000, 5); // 60 W * 180 s = 0.003 kWh
}

#[test]
fn recompute_after_tariff_correction_changes_cost_not_energy() {
    let mut s = Storage::open_in_memory().unwrap();
    let old = tariff(1, "0.20", "0.10");
    s.record_span(&span(sofia(2026, 5, 4, 10, 0), sofia(2026, 5, 4, 12, 0), PowerState::Active, "100"), &old).unwrap();
    let (from, to) = (sofia(2026, 5, 4, 0, 0), sofia(2026, 5, 5, 0, 0));
    let before = s.summary(from, to).unwrap();

    let fixed = tariff(2, "0.30", "0.15");
    let rep = s.recompute_costs(from, to, &fixed).unwrap();
    assert_eq!(rep.spans, 1);
    assert_eq!(rep.minutes_deleted, 120);
    let after = s.summary(from, to).unwrap();
    assert_eq!(before.total.kwh_nano, after.total.kwh_nano);
    near(before.total.cost_nano, 40_000_000, 120); // 0.2 kWh * 0.20
    near(after.total.cost_nano, 60_000_000, 120); // 0.2 kWh * 0.30
}

#[test]
fn recompute_of_part_of_a_span_only_touches_that_part() {
    let mut s = Storage::open_in_memory().unwrap();
    let old = tariff(1, "0.20", "0.10");
    s.record_span(&span(sofia(2026, 5, 4, 10, 0), sofia(2026, 5, 4, 12, 0), PowerState::Active, "100"), &old).unwrap();
    let fixed = tariff(2, "0.40", "0.20");
    s.recompute_costs(sofia(2026, 5, 4, 11, 0), sofia(2026, 5, 4, 12, 0), &fixed).unwrap();
    let sum = s.summary(sofia(2026, 5, 4, 0, 0), sofia(2026, 5, 5, 0, 0)).unwrap();
    // first hour at 0.20, second at 0.40: 0.1*0.2 + 0.1*0.4
    near(sum.total.cost_nano, 20_000_000 + 40_000_000, 120);
    assert_eq!(sum.total.seconds, 7200, "no minutes lost or doubled");
}

#[test]
fn reestimate_after_calibration_change_updates_watts_layer_and_cost() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.20");
    let inputs = Inputs { cpu_load_pct: 50.0, ..Inputs::new(PowerState::Idle) };
    let default_est = Estimator::new(EstimatorConfig::default()).unwrap();
    let est = default_est.estimate(&inputs);
    let (start, end) = (sofia(2026, 5, 4, 10, 0), sofia(2026, 5, 4, 11, 0));
    s.record_span(
        &Span { start, end, inputs: inputs.clone(), watts: est.watts, layer: est.layer, billable: true, source: SpanSource::Simulated },
        &t,
    )
    .unwrap();
    assert_eq!(s.spans_between(start, end).unwrap()[0].layer, Layer::Default);
    let before = s.summary(start, end).unwrap().total.cost_nano;

    let cfg = EstimatorConfig { calibration: Some(Calibration::new(d("40"), d("160")).unwrap()), ..EstimatorConfig::default() }; // 50% -> 100 W
    let calibrated = Estimator::new(cfg).unwrap();
    let rep = s.reestimate(start, end, &calibrated, &t).unwrap();
    assert_eq!(rep.spans, 1);
    let spans = s.spans_between(start, end).unwrap();
    assert_eq!(spans[0].watts, d("100.00"));
    assert_eq!(spans[0].layer, Layer::Calibrated);
    let after = s.summary(start, end).unwrap().total;
    near(after.kwh_nano, 100_000_000, 60);
    near(after.cost_nano, 20_000_000, 60); // 0.1 kWh * 0.20
    assert_ne!(before, after.cost_nano);
}

// ---------- gap ingestion ----------

fn standby_report() -> GapReport {
    let xml = std::fs::read_to_string(format!(
        "{}/../gap-classifier/tests/fixtures/real_modern_standby_2026-11-05.xml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let evs = parse_events_xml(&xml).unwrap();
    let g = Gap { start: Utc.with_ymd_and_hms(2026, 11, 5, 22, 28, 0).unwrap(), end: Utc.with_ymd_and_hms(2026, 11, 6, 15, 27, 0).unwrap() };
    classify(g, &evs, &Config::default())
}

#[test]
fn real_modern_standby_gap_is_priced_by_its_active_share() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    let est = Estimator::new(EstimatorConfig::default()).unwrap(); // idle model ~ 29.41 W
    let report = standby_report();

    let res = s.ingest_gap(&report, &est, &Assumptions::default(), true, &t).unwrap();
    assert_eq!(res.spans, 2); // sleep + untracked tail
    assert!(!res.already_ingested);

    let sleep = &s.spans_between(report.start, report.end).unwrap()[0];
    assert_eq!(sleep.inputs.state, PowerState::Sleep);
    let permille = sleep.inputs.sleep_active_permille.unwrap();
    assert!((940..=990).contains(&permille));
    // a flat 1 W sleep would be ~17 Wh; the active share makes it ~29 W * ~17 h
    assert!(sleep.watts > d("20"), "sleep priced at {} W", sleep.watts);
    let sum = s.summary(report.start, report.end).unwrap();
    assert!(sum.by_state[&PowerState::Sleep].kwh() > d("0.4"));

    // idempotent
    let again = s.ingest_gap(&report, &est, &Assumptions::default(), true, &t).unwrap();
    assert!(again.already_ingested);
    assert_eq!(s.summary(report.start, report.end).unwrap().total, sum.total);
    assert!(s.count_gaps().unwrap() >= 2);
}

#[test]
fn gap_on_battery_keeps_energy_but_costs_nothing() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    let est = Estimator::new(EstimatorConfig::default()).unwrap();
    let (a, b) = (sofia(2026, 5, 4, 10, 0), sofia(2026, 5, 4, 12, 0));
    let report = GapReport {
        start: a,
        end: b,
        segments: vec![Segment {
            start: a,
            end: b,
            kind: GapKind::Sleep,
            confidence: Confidence::High,
            evidence: vec![],
            sleep: Some(SleepDetails { duration_us: Some(7_200_000_000), active_us: Some(0), on_ac: Some(false), battery_delta_mwh: Some(-500) }),
            note: None,
        }],
    };
    s.ingest_gap(&report, &est, &Assumptions::default(), true, &t).unwrap();
    let sum = s.summary(a, b).unwrap();
    assert!(sum.total.kwh_nano > 0);
    assert_eq!(sum.total.cost_nano, 0);
}

#[test]
fn crash_and_unknown_use_the_configured_assumptions() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    let est = Estimator::new(EstimatorConfig::default()).unwrap();
    let (a, b) = (sofia(2026, 5, 4, 10, 0), sofia(2026, 5, 4, 12, 0));
    let seg = |kind| Segment { start: a, end: b, kind, confidence: Confidence::Low, evidence: vec![], sleep: None, note: None };
    let rep = |kind| GapReport { start: a, end: b, segments: vec![seg(kind)] };

    s.ingest_gap(&rep(GapKind::Crash), &est, &Assumptions::default(), true, &t).unwrap();
    assert_eq!(s.spans_between(a, b).unwrap()[0].inputs.state, PowerState::Off);

    let mut s2 = Storage::open_in_memory().unwrap();
    let idle_crash = Assumptions { crash: PowerState::Idle, ..Assumptions::default() };
    s2.ingest_gap(&rep(GapKind::Crash), &est, &idle_crash, true, &t).unwrap();
    assert_eq!(s2.spans_between(a, b).unwrap()[0].inputs.state, PowerState::Idle);
}

// ---------- compaction ----------

fn fill_minutes(s: &mut Storage, t: &TariffSet, from: DateTime<Utc>, minutes: i64) {
    for m in 0..minutes {
        let a = from + Duration::minutes(m);
        let state = if m % 3 == 0 { PowerState::Active } else { PowerState::Idle };
        s.record_span(&span(a, a + Duration::minutes(1), state, "80"), t).unwrap();
    }
}

#[test]
fn compaction_preserves_totals_and_removes_minute_rows() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    let start = sofia(2026, 5, 1, 0, 0);
    fill_minutes(&mut s, &t, start, 3 * 1440);
    let end = sofia(2026, 5, 4, 0, 0);
    let before = s.summary(start, end).unwrap();
    assert_eq!(before.total.seconds, 3 * 86_400);

    // compact everything before 2026-05-03 local midnight
    let rep = s.compact(sofia(2026, 5, 3, 13, 0), SOFIA).unwrap();
    assert_eq!(rep.compacted_until, Some(sofia(2026, 5, 3, 0, 0)));
    assert_eq!(rep.minute_rows_removed, 2 * 1440);
    assert!(rep.hourly_rows_written > 0 && rep.daily_rows_written > 0);

    let after = s.summary(start, end).unwrap();
    assert_eq!(after.total, before.total, "aggregates must reproduce the totals exactly");
    assert_eq!(after.by_state, before.by_state);
    assert_eq!(after.by_period, before.by_period);

    // daily view: 3 days, each with 24 h
    let days = s.daily_totals(start, end, SOFIA).unwrap();
    assert_eq!(days.len(), 3);
    assert!(days.values().all(|t| t.seconds == 86_400));

    // idempotent
    let again = s.compact(sofia(2026, 5, 3, 13, 0), SOFIA).unwrap();
    assert_eq!(again, CompactionReport::default());
    assert_eq!(s.summary(start, end).unwrap().total, before.total);
}

#[test]
fn recompute_cannot_rewrite_compacted_history() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    let start = sofia(2026, 5, 1, 0, 0);
    fill_minutes(&mut s, &t, start, 2 * 1440);
    s.compact(sofia(2026, 5, 2, 12, 0), SOFIA).unwrap();
    let before = s.summary(start, sofia(2026, 5, 3, 0, 0)).unwrap().total;

    let rep = s.recompute_costs(start, sofia(2026, 5, 3, 0, 0), &tariff(2, "0.50", "0.50")).unwrap();
    assert!(rep.clipped_by_compaction);
    let after = s.summary(start, sofia(2026, 5, 3, 0, 0)).unwrap().total;
    assert_eq!(after.kwh_nano, before.kwh_nano);
    // day 1 (compacted) unchanged, day 2 repriced at 0.50
    assert!(after.cost_nano > before.cost_nano);
    let days = s.daily_totals(start, sofia(2026, 5, 3, 0, 0), SOFIA).unwrap();
    let d1 = days[&NaiveDate::from_ymd_opt(2026, 5, 1).unwrap()].cost_nano;
    let d2 = days[&NaiveDate::from_ymd_opt(2026, 5, 2).unwrap()].cost_nano;
    assert!(d2 > d1 * 2, "day 2 ({d2}) should now cost much more than the compacted day 1 ({d1})");
}

#[test]
fn compacting_a_dst_day_keeps_23_real_hours_in_one_local_day() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    // Sofia 2026-03-29 has 23 hours; one span over the whole local day + the next day
    let (a, b, c) = (sofia(2026, 3, 29, 0, 0), sofia(2026, 3, 30, 0, 0), sofia(2026, 3, 31, 0, 0));
    assert_eq!(b - a, Duration::hours(23));
    s.record_span(&span(a, b, PowerState::Idle, "50"), &t).unwrap();
    s.record_span(&span(b, c, PowerState::Idle, "50"), &t).unwrap();
    s.compact(c, SOFIA).unwrap();
    let days = s.daily_totals(a, c, SOFIA).unwrap();
    assert_eq!(days[&NaiveDate::from_ymd_opt(2026, 3, 29).unwrap()].seconds, 23 * 3600);
    assert_eq!(days[&NaiveDate::from_ymd_opt(2026, 3, 30).unwrap()].seconds, 24 * 3600);
}

#[test]
fn a_long_span_straddling_the_compaction_cutoff_stays_consistent() {
    let mut s = Storage::open_in_memory().unwrap();
    let t = tariff(1, "0.20", "0.10");
    let (start, end) = (sofia(2026, 5, 1, 18, 0), sofia(2026, 5, 3, 0, 0)); // 30 h gap-like span
    s.record_span(&span(start, end, PowerState::Sleep, "20"), &t).unwrap();
    let before = s.summary(start, end).unwrap();
    s.compact(sofia(2026, 5, 2, 12, 0), SOFIA).unwrap(); // cutoff = May 2 00:00 local
    assert_eq!(s.summary(start, end).unwrap().total, before.total);
    // repricing the part after the cutoff leaves the compacted part alone
    s.recompute_costs(start, end, &tariff(2, "0.40", "0.20")).unwrap();
    let after = s.summary(start, end).unwrap().total;
    assert_eq!(after.seconds, before.total.seconds);
    assert!(after.cost_nano > before.total.cost_nano && after.cost_nano < before.total.cost_nano * 2);
}

