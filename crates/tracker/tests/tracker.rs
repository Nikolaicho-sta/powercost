//! Tracker tests: mock sensors, fake clock, fake event log. Tariff is SYNTHETIC flat 0.20 EUR/kWh.

use chrono::{DateTime, Duration, TimeZone, Utc};
use cost_engine::TariffSet;
use estimator::{Estimator, EstimatorConfig, Layer};
use gap_classifier::heartbeat::{Heartbeat, PowerSource};
use gap_classifier::{parse_events_xml, EventSource, PowerEvent};
use pc_core::PowerState;
use sensors::mock::*;
use sensors::{GpuReading, PowerSourceReading, SensorHub};
use serde_json::json;
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
use std::time::Duration as StdDuration;
use storage::Storage;
use tariff_engine::Tariff;
use tracker::*;

#[derive(Clone, Default)]
struct FakeEvents {
    events: Vec<PowerEvent>,
    calls: Arc<AtomicUsize>,
}

impl EventSource for FakeEvents {
    fn events(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> std::result::Result<Vec<PowerEvent>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.events.iter().filter(|e| e.time >= from && e.time <= to).cloned().collect())
    }
}

fn t(h: u32, m: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 4, h, m, s).unwrap()
}

fn tariffs() -> TariffSet {
    TariffSet::single(
        Tariff::from_value(json!({
            "schema_version": 1, "id": "flat", "name": "flat", "country": "BG", "currency": "EUR",
            "timezone": "Europe/Sofia", "vat_percent": "0", "valid_from": "2026-01-01",
            "components": [{"id": "e", "kind": "energy", "vat_inclusive": true, "per_kwh": "0.20"}],
            "periods": [{"id": "flat", "label": "Flat"}],
            "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
                {"days": ["all"], "ranges": [{"start": "00:00", "end": "24:00"}], "period": "flat"}]}]}
        }))
        .unwrap(),
    )
}

fn ac(on: bool, charging: bool, rate_mw: i32) -> PowerSourceReading {
    PowerSourceReading { has_battery: true, ac_online: Some(on), charging, battery_percent: Some(80), battery_rate_mw: Some(rate_mw) }
}

fn hub(idle_secs: u64, cpu_load: f64, source: PowerSourceReading, cpu_power: Option<f64>) -> SensorHub {
    SensorHub {
        cpu_power: cpu_power.map(|w| Box::new(MockCpuPower(Some(w))) as _),
        gpu: cpu_power.map(|_| Box::new(MockGpu(Some(GpuReading { watts: 30.0, util_percent: 10.0 }))) as _),
        cpu_load: Some(Box::new(MockCpuLoad(cpu_load))),
        input: Some(Box::new(MockInput(StdDuration::from_secs(idle_secs)))),
        power_source: Some(Box::new(MockPowerSource(source))),
        missing: vec![],
    }
}

fn tracker_with(hub: SensorHub, events: FakeEvents, store: Storage) -> Tracker {
    Tracker::new(
        store,
        hub,
        Estimator::new(EstimatorConfig::default()).unwrap(),
        tariffs(),
        Box::new(events),
        TrackerConfig::default(),
        "session-A".into(),
    )
}

fn new_tracker(hub: SensorHub) -> Tracker {
    tracker_with(hub, FakeEvents::default(), Storage::open_in_memory().unwrap())
}

fn run(tr: &mut Tracker, from: DateTime<Utc>, secs: i64, step: i64) {
    let mut s = 0;
    while s <= secs {
        tr.tick(from + Duration::seconds(s)).unwrap();
        s += step;
    }
}

fn day() -> (DateTime<Utc>, DateTime<Utc>) {
    (t(0, 0, 0), t(23, 59, 59) + Duration::seconds(1))
}

// ---------- per-minute spans ----------

#[test]
fn a_minute_of_samples_becomes_one_span_when_the_next_minute_starts() {
    let mut tr = new_tracker(hub(0, 5.0, ac(true, false, 0), None));
    run(&mut tr, t(12, 0, 5), 50, 10); // 12:00:05 .. 12:00:55
    assert!(tr.store().spans_between(t(12, 0, 0), t(13, 0, 0)).unwrap().is_empty(), "minute still open");
    let rep = tr.tick(t(12, 1, 5)).unwrap();
    assert_eq!(rep.spans_written, 1);
    let spans = tr.store().spans_between(t(12, 0, 0), t(12, 1, 0)).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].start, spans[0].end), (t(12, 0, 5), t(12, 1, 0)));
    assert_eq!(spans[0].inputs.state, PowerState::Active); // input 0 s ago
    assert!(spans[0].billable);
}

#[test]
fn spans_tile_continuous_tracking_without_gaps_or_overlaps() {
    let mut tr = new_tracker(hub(0, 5.0, ac(true, false, 0), None));
    run(&mut tr, t(12, 0, 5), 600, 10);
    tr.shutdown(t(12, 10, 5)).unwrap();
    let spans = tr.store().spans_between(t(12, 0, 0), t(13, 0, 0)).unwrap();
    assert_eq!(spans.first().unwrap().start, t(12, 0, 5));
    assert_eq!(spans.last().unwrap().end, t(12, 10, 5));
    for w in spans.windows(2) {
        assert_eq!(w[0].end, w[1].start);
    }
    let (a, b) = day();
    let sum = tr.store().summary(a, b).unwrap();
    assert_eq!(sum.total.seconds, 600);
}

#[test]
fn idle_when_no_input_and_low_load() {
    let mut tr = new_tracker(hub(3600, 2.0, ac(true, false, 0), None));
    run(&mut tr, t(12, 0, 5), 130, 10);
    let spans = tr.store().spans_between(t(12, 0, 0), t(13, 0, 0)).unwrap();
    assert!(!spans.is_empty() && spans.iter().all(|s| s.inputs.state == PowerState::Idle));
}

#[test]
fn heavy_load_without_input_is_active() {
    let mut tr = new_tracker(hub(3600, 90.0, ac(true, false, 0), None));
    run(&mut tr, t(12, 0, 5), 130, 10);
    assert!(tr.store().spans_between(t(12, 0, 0), t(13, 0, 0)).unwrap().iter().all(|s| s.inputs.state == PowerState::Active));
}

// ---------- estimation & billing ----------

#[test]
fn sensor_readings_select_the_sensors_layer_and_default_otherwise() {
    let mut with = new_tracker(hub(0, 20.0, ac(true, false, 0), Some(18.0)));
    run(&mut with, t(12, 0, 5), 70, 10);
    let s = &with.store().spans_between(t(12, 0, 0), t(13, 0, 0)).unwrap()[0];
    assert_eq!(s.layer, Layer::Sensors);
    assert_eq!(s.inputs.cpu_power_w, Some(18.0));
    assert_eq!(with.current().unwrap().layer, "sensors");

    let mut without = new_tracker(hub(0, 20.0, ac(true, false, 0), None));
    run(&mut without, t(12, 0, 5), 70, 10);
    assert_eq!(without.store().spans_between(t(12, 0, 0), t(13, 0, 0)).unwrap()[0].layer, Layer::Default);
}

#[test]
fn battery_minutes_keep_their_energy_but_cost_nothing() {
    let mut tr = new_tracker(hub(0, 20.0, ac(false, false, -15_000), None));
    run(&mut tr, t(12, 0, 5), 130, 10);
    let (a, b) = day();
    let sum = tr.store().summary(a, b).unwrap();
    assert!(sum.total.kwh_nano > 0);
    assert_eq!(sum.total.cost_nano, 0);
    assert_eq!(tr.current().unwrap().on_ac, Some(false));
}

#[test]
fn charging_on_ac_adds_to_the_estimated_draw() {
    let mut idle = new_tracker(hub(0, 20.0, ac(true, false, 0), Some(18.0)));
    let mut charging = new_tracker(hub(0, 20.0, ac(true, true, 12_000), Some(18.0)));
    run(&mut idle, t(12, 0, 5), 70, 10);
    run(&mut charging, t(12, 0, 5), 70, 10);
    let w = |tr: &Tracker| tr.store().spans_between(t(12, 0, 0), t(13, 0, 0)).unwrap()[0].watts;
    assert!(w(&charging) > w(&idle), "charging {} vs {}", w(&charging), w(&idle));
}

// ---------- heartbeat & events ----------

#[test]
fn heartbeat_is_written_on_schedule_and_on_shutdown() {
    let mut tr = new_tracker(hub(0, 5.0, ac(true, false, 0), None));
    run(&mut tr, t(12, 0, 5), 100, 10);
    let hb = tr.store().last_heartbeat().unwrap().unwrap();
    assert!(t(12, 1, 45) - hb.time <= Duration::seconds(30), "last heartbeat {}", hb.time);
    assert_eq!(hb.session_id, "session-A");
    assert_eq!(hb.source, PowerSource::Ac);
    tr.shutdown(t(12, 1, 50)).unwrap();
    assert_eq!(tr.store().last_heartbeat().unwrap().unwrap().time, t(12, 1, 50));
}

#[test]
fn power_events_are_copied_to_the_database_periodically_not_every_tick() {
    let events = FakeEvents { events: vec![PowerEvent::new(t(11, 59, 0), "Microsoft-Windows-Kernel-Power", 506)], ..Default::default() };
    let calls = events.calls.clone();
    let mut tr = tracker_with(hub(0, 5.0, ac(true, false, 0), None), events, Storage::open_in_memory().unwrap());
    tr.startup(t(12, 0, 0)).unwrap();
    let after_startup = calls.load(Ordering::SeqCst);
    run(&mut tr, t(12, 0, 5), 600, 10); // 10 minutes, persist interval is 30
    assert_eq!(calls.load(Ordering::SeqCst), after_startup, "no event-log queries during normal ticking");
    tr.tick(t(12, 31, 0)).unwrap();
    assert!(calls.load(Ordering::SeqCst) > after_startup);
    let saved = tr.store().power_events_between(t(0, 0, 0), t(23, 0, 0)).unwrap();
    assert_eq!(saved.len(), 1);
}

// ---------- startup gap recovery ----------

fn restart_events() -> Vec<PowerEvent> {
    let xml = std::fs::read_to_string(format!(
        "{}/../gap-classifier/tests/fixtures/real_restart_2026-11-03.xml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    parse_events_xml(&xml).unwrap()
}

#[test]
fn startup_after_a_real_restart_stores_the_gap_and_the_events() {
    let store = Storage::open_in_memory().unwrap();
    let last = Utc.with_ymd_and_hms(2026, 11, 3, 16, 11, 30).unwrap();
    store
        .write_heartbeat(&Heartbeat { time: last, session_id: "old-session".into(), state: PowerState::Active, source: PowerSource::Ac })
        .unwrap();
    let mut tr = tracker_with(
        hub(0, 5.0, ac(true, false, 0), None),
        FakeEvents { events: restart_events(), ..Default::default() },
        store,
    );
    let now = Utc.with_ymd_and_hms(2026, 11, 3, 16, 13, 0).unwrap();
    let rep = tr.startup(now).unwrap();
    let gap = rep.gap.expect("a gap was found");
    assert_eq!(gap.start, last);
    assert!(rep.spans_added >= 3 && rep.events_saved >= 8);
    // the whole gap is accounted for, nothing double counted
    // (summary works on whole minute rows, so use a minute-aligned range)
    let sum = tr.store().summary(Utc.with_ymd_and_hms(2026, 11, 3, 16, 11, 0).unwrap(), Utc.with_ymd_and_hms(2026, 11, 3, 16, 13, 0).unwrap()).unwrap();
    assert_eq!(sum.total.seconds, 90);
    // the old session's heartbeat was replaced by ours
    assert_eq!(tr.store().last_heartbeat().unwrap().unwrap().session_id, "session-A");
    // tracking continues seamlessly from the startup instant
    tr.tick(now + Duration::seconds(5)).unwrap();
    tr.shutdown(now + Duration::seconds(65)).unwrap();
    let sum = tr.store().summary(Utc.with_ymd_and_hms(2026, 11, 3, 16, 11, 0).unwrap(), Utc.with_ymd_and_hms(2026, 11, 3, 16, 15, 0).unwrap()).unwrap();
    assert_eq!(sum.total.seconds, 90 + 65);
}

#[test]
fn first_run_has_no_gap() {
    let mut tr = new_tracker(hub(0, 5.0, ac(true, false, 0), None));
    let rep = tr.startup(t(12, 0, 0)).unwrap();
    assert!(rep.gap.is_none());
}

#[test]
fn startup_after_a_crash_with_no_events_uses_the_off_assumption() {
    let store = Storage::open_in_memory().unwrap();
    store
        .write_heartbeat(&Heartbeat { time: t(1, 0, 0), session_id: "old".into(), state: PowerState::Idle, source: PowerSource::Ac })
        .unwrap();
    let mut tr = tracker_with(hub(0, 5.0, ac(true, false, 0), None), FakeEvents::default(), store);
    let rep = tr.startup(t(9, 0, 0)).unwrap();
    assert_eq!(rep.gap.unwrap().segments[0].kind, gap_classifier::GapKind::Unknown);
    let sum = tr.store().summary(t(1, 0, 0), t(9, 0, 0)).unwrap();
    assert_eq!(sum.by_state[&PowerState::Off].seconds, 8 * 3600);
}

// ---------- in-process gaps ----------

#[test]
fn sleep_while_running_is_recovered_from_events_and_priced_by_active_share() {
    let events = FakeEvents {
        events: vec![
            PowerEvent::new(t(12, 1, 0), "Microsoft-Windows-Kernel-Power", 506),
            PowerEvent::new(t(13, 59, 58), "Microsoft-Windows-Kernel-Power", 507)
                .with("DurationInUs", "7138000000")
                .with("ActiveResidencyInUs", "6900000000")
                .with("PowerStateAc", "true"),
        ],
        ..Default::default()
    };
    let mut tr = tracker_with(hub(0, 5.0, ac(true, false, 0), None), events, Storage::open_in_memory().unwrap());
    run(&mut tr, t(12, 0, 5), 50, 10);
    let rep = tr.tick(t(14, 0, 0)).unwrap(); // laptop slept ~2 h, process resumed
    assert!(rep.gap_handled);
    tr.shutdown(t(14, 1, 0)).unwrap();
    let sum = tr.store().summary(t(12, 0, 0), t(15, 0, 0)).unwrap();
    let sleep = sum.by_state[&PowerState::Sleep];
    assert!((7100..=7200).contains(&sleep.seconds), "sleep seconds {}", sleep.seconds);
    // ~96.7% active share -> far more than 1 W for 2 h
    assert!(sleep.kwh_nano > 30_000_000, "sleep kWh nano {}", sleep.kwh_nano);
    // continuous coverage from first sample to shutdown
    assert_eq!(sum.total.seconds, (t(14, 1, 0) - t(12, 0, 5)).num_seconds());
}

#[test]
fn a_stall_without_any_events_is_treated_as_idle_not_as_power_off() {
    let mut tr = new_tracker(hub(0, 5.0, ac(true, false, 0), None));
    run(&mut tr, t(12, 0, 5), 20, 10);
    let rep = tr.tick(t(12, 5, 0)).unwrap(); // 5 min silence, no events
    assert!(rep.gap_handled);
    tr.shutdown(t(12, 5, 30)).unwrap();
    let sum = tr.store().summary(t(12, 0, 0), t(13, 0, 0)).unwrap();
    assert!(!sum.by_state.contains_key(&PowerState::Off));
    assert!(sum.by_state[&PowerState::Idle].seconds >= 250);
}

#[test]
fn normal_ticking_never_reports_a_gap() {
    let mut tr = new_tracker(hub(0, 5.0, ac(true, false, 0), None));
    let mut gaps = 0;
    let mut s = 0;
    while s < 3600 {
        if tr.tick(t(12, 0, 5) + Duration::seconds(s)).unwrap().gap_handled {
            gaps += 1;
        }
        s += 10;
    }
    assert_eq!(gaps, 0);
}


