//! Gap-classifier tests. `real_*` fixtures are sanitised exports from the reference laptop
//! (Windows 11, Modern Standby). Everything built with `ev(...)` is SYNTHETIC, written from
//! documented event semantics because the reference machine produced no such events.

use chrono::{DateTime, Duration, TimeZone, Utc};
use gap_classifier::heartbeat::*;
use gap_classifier::*;
use pc_core::PowerState;
use std::fs;

const KP: &str = "Microsoft-Windows-Kernel-Power";
const KG: &str = "Microsoft-Windows-Kernel-General";
const KB: &str = "Microsoft-Windows-Kernel-Boot";

fn t(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

fn ev(time: DateTime<Utc>, provider: &str, id: u32) -> PowerEvent {
    PowerEvent::new(time, provider, id)
}

fn fixture(name: &str) -> Vec<PowerEvent> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    parse_events_xml(&fs::read_to_string(path).unwrap()).unwrap()
}

fn kinds(r: &GapReport) -> Vec<GapKind> {
    r.segments.iter().map(|s| s.kind).collect()
}

fn gap(a: DateTime<Utc>, b: DateTime<Utc>) -> Gap {
    Gap { start: a, end: b }
}

fn run(g: Gap, events: &[PowerEvent]) -> GapReport {
    let r = classify(g, events, &Config::default());
    // invariants: segments tile the gap exactly, in order, no overlap
    assert_eq!(r.segments.first().unwrap().start, g.start);
    assert_eq!(r.segments.last().unwrap().end, g.end);
    for w in r.segments.windows(2) {
        assert_eq!(w[0].end, w[1].start, "segments must be contiguous: {r:#?}");
    }
    r
}

// ---------- real fixtures ----------

#[test]
fn parses_real_restart_export() {
    let evs = fixture("real_restart_2026-11-03.xml");
    assert_eq!(evs.len(), 8);
    assert_eq!((evs[0].short_provider(), evs[0].id), ("User32", 1074));
    assert_eq!(evs[0].get("param5"), Some("restart"));
    assert!(evs.windows(2).all(|w| w[0].time <= w[1].time));
    let boot20 = evs.iter().find(|e| e.short_provider() == "Kernel-Boot" && e.id == 20).unwrap();
    assert_eq!(boot20.get("LastShutdownGood"), Some("true"));
}

#[test]
fn real_restart_is_classified_as_restart_with_on_time_either_side() {
    let evs = fixture("real_restart_2026-11-03.xml");
    // last heartbeat 16:11:30, tracker came back 16:13:00
    let r = run(gap(t(2026, 11, 3, 16, 11, 30), t(2026, 11, 3, 16, 13, 0)), &evs);
    assert_eq!(kinds(&r), vec![GapKind::OnUntracked, GapKind::Restart, GapKind::OnUntracked]);
    let restart = &r.segments[1];
    assert_eq!(restart.confidence, Confidence::High);
    // shutdown begins at EventLog 6006 (16:11:59), boot at Kernel-General 12 (16:12:27)
    assert_eq!(restart.start.format("%H:%M:%S").to_string(), "16:11:59");
    assert_eq!(restart.end.format("%H:%M:%S").to_string(), "16:12:27");
    assert!(restart.evidence.iter().any(|e| e.id == 6006) && restart.evidence.iter().any(|e| e.id == 12));
}

#[test]
fn real_modern_standby_chain_merges_into_one_sleep_with_active_share() {
    let evs = fixture("real_modern_standby_2026-11-05.xml");
    assert_eq!(evs.len(), 15);
    let g = gap(t(2026, 11, 5, 22, 28, 0), t(2026, 11, 6, 15, 27, 0));
    let r = run(g, &evs);
    // 507/506 pairs 1 ms apart and short wake blips fold into a single sleep until the last 507
    assert_eq!(kinds(&r), vec![GapKind::Sleep, GapKind::OnUntracked]);
    let sleep = &r.segments[0];
    assert_eq!(sleep.start, g.start);
    assert_eq!(sleep.end.format("%H:%M:%S").to_string(), "15:26:46");
    let d = sleep.sleep.unwrap();
    assert_eq!(d.on_ac, Some(true));
    assert_eq!(d.battery_delta_mwh, Some(0));
    // The laptop spent ~96% of this "sleep" actively running (lid closed, external monitor, audio):
    let pm = d.active_permille().unwrap();
    assert!((940..=990).contains(&pm), "active share {pm} per-mille");
}

#[test]
fn sleep_that_began_before_the_gap_is_scaled_to_the_part_inside() {
    let evs = fixture("real_modern_standby_2026-11-05.xml");
    // Gap starts 1 s before the first 507 (22:28:01.70) whose DurationInUs is 13,031 s.
    let r = run(gap(t(2026, 11, 5, 22, 28, 0), t(2026, 11, 5, 22, 28, 30)), &evs);
    let first = &r.segments[0];
    assert_eq!(first.kind, GapKind::Sleep);
    let d = first.sleep.unwrap();
    // not the full 13,031 s: only what falls inside the 30 s gap
    assert!(d.duration_us.unwrap() < 31_000_000, "duration {:?}", d.duration_us);
}

// ---------- synthetic scenarios ----------

#[test]
fn classic_s3_sleep_and_resume() {
    let evs = vec![
        ev(t(2026, 5, 1, 22, 0, 5), KP, 42).with("TargetState", "3"),
        ev(t(2026, 5, 2, 7, 0, 0), KP, 107),
    ];
    let r = run(gap(t(2026, 5, 1, 22, 0, 0), t(2026, 5, 2, 7, 0, 10)), &evs);
    assert_eq!(kinds(&r), vec![GapKind::OnUntracked, GapKind::Sleep, GapKind::OnUntracked]);
    assert_eq!(r.segments[1].confidence, Confidence::High);
}

#[test]
fn hibernate_then_resume_from_hibernate() {
    let evs = vec![
        ev(t(2026, 5, 1, 22, 0, 5), KP, 42).with("TargetState", "4"),
        ev(t(2026, 5, 2, 6, 0, 0), KG, 12),
        ev(t(2026, 5, 2, 6, 0, 0), KB, 27).with("BootType", "2"),
    ];
    let r = run(gap(t(2026, 5, 1, 22, 0, 0), t(2026, 5, 2, 6, 0, 30)), &evs);
    assert_eq!(kinds(&r), vec![GapKind::OnUntracked, GapKind::Hibernate, GapKind::OnUntracked]);
    assert_eq!(r.segments[1].kind.power_state(&Assumptions::default()), PowerState::Hibernate);
}

#[test]
fn fast_startup_shutdown_is_off_and_noted() {
    let evs = vec![
        ev(t(2026, 5, 1, 22, 0, 10), "User32", 1074).with("param5", "power off"),
        ev(t(2026, 5, 1, 22, 0, 20), "EventLog", 6006),
        ev(t(2026, 5, 1, 22, 0, 25), KP, 109),
        ev(t(2026, 5, 1, 22, 0, 26), KG, 13),
        ev(t(2026, 5, 2, 8, 0, 0), KG, 12),
        ev(t(2026, 5, 2, 8, 0, 0), KB, 27).with("BootType", "1"),
        ev(t(2026, 5, 2, 8, 0, 0), KB, 20).with("LastShutdownGood", "true"),
    ];
    let r = run(gap(t(2026, 5, 1, 22, 0, 0), t(2026, 5, 2, 8, 0, 30)), &evs);
    assert_eq!(kinds(&r), vec![GapKind::OnUntracked, GapKind::Shutdown, GapKind::OnUntracked]);
    assert!(r.segments[1].note.as_deref().unwrap().contains("Fast Startup"));
    assert_eq!(r.segments[1].kind.power_state(&Assumptions::default()), PowerState::Off);
}

#[test]
fn crash_is_detected_from_boot_markers_and_spans_from_last_heartbeat() {
    let boot = t(2026, 5, 2, 9, 0, 0);
    let evs = vec![
        ev(boot, KG, 12),
        ev(boot, KB, 20).with("LastShutdownGood", "false"),
        ev(boot + Duration::seconds(40), KP, 41),
        ev(boot + Duration::seconds(45), "EventLog", 6008),
    ];
    let r = run(gap(t(2026, 5, 2, 3, 0, 0), boot + Duration::seconds(60)), &evs);
    assert_eq!(kinds(&r), vec![GapKind::Crash, GapKind::OnUntracked]);
    let crash = &r.segments[0];
    assert_eq!(crash.confidence, Confidence::High);
    assert_eq!(crash.start, t(2026, 5, 2, 3, 0, 0));
    assert_eq!(crash.end, boot);
    assert!(crash.evidence.iter().any(|e| e.id == 41));
}

#[test]
fn boot_without_shutdown_and_without_markers_is_a_medium_confidence_crash() {
    let boot = t(2026, 5, 2, 9, 0, 0);
    let r = run(gap(t(2026, 5, 2, 3, 0, 0), boot + Duration::seconds(30)), &[ev(boot, KG, 12)]);
    assert_eq!(r.segments[0].kind, GapKind::Crash);
    assert_eq!(r.segments[0].confidence, Confidence::Medium);
}

#[test]
fn sleep_followed_by_cold_boot_means_power_was_lost() {
    let evs = vec![ev(t(2026, 5, 1, 23, 0, 0), KP, 42).with("TargetState", "3"), ev(t(2026, 5, 2, 8, 0, 0), KG, 12)];
    let r = run(gap(t(2026, 5, 1, 22, 59, 50), t(2026, 5, 2, 8, 0, 20)), &evs);
    assert_eq!(kinds(&r), vec![GapKind::OnUntracked, GapKind::Crash, GapKind::OnUntracked]);
}

#[test]
fn no_events_gives_unknown_low_confidence() {
    let r = run(gap(t(2026, 5, 1, 0, 0, 0), t(2026, 5, 3, 0, 0, 0)), &[]);
    assert_eq!(kinds(&r), vec![GapKind::Unknown]);
    assert_eq!(r.segments[0].confidence, Confidence::Low);
    assert_eq!(GapKind::Unknown.power_state(&Assumptions::default()), PowerState::Off);
    let a = Assumptions { unknown: PowerState::Idle, ..Default::default() };
    assert_eq!(GapKind::Unknown.power_state(&a), PowerState::Idle);
}

#[test]
fn wake_event_without_matching_enter_infers_sleep_from_gap_start() {
    let r = run(gap(t(2026, 5, 1, 22, 0, 0), t(2026, 5, 2, 7, 0, 30)), &[ev(t(2026, 5, 2, 7, 0, 0), KP, 107)]);
    assert_eq!(kinds(&r), vec![GapKind::Sleep, GapKind::OnUntracked]);
    assert_eq!(r.segments[0].confidence, Confidence::Medium);
}

#[test]
fn sleep_without_wake_runs_to_gap_end_with_low_confidence() {
    let r = run(gap(t(2026, 5, 1, 22, 0, 0), t(2026, 5, 2, 7, 0, 0)), &[ev(t(2026, 5, 1, 22, 0, 3), KP, 506)]);
    assert_eq!(kinds(&r), vec![GapKind::OnUntracked, GapKind::Sleep]);
    assert_eq!(r.segments[1].confidence, Confidence::Low);
}

#[test]
fn events_outside_the_gap_are_ignored() {
    let evs = vec![ev(t(2026, 4, 1, 0, 0, 0), KP, 506), ev(t(2026, 4, 1, 1, 0, 0), KP, 507)];
    let r = run(gap(t(2026, 5, 1, 0, 0, 0), t(2026, 5, 1, 1, 0, 0)), &evs);
    assert_eq!(kinds(&r), vec![GapKind::Unknown]);
}

#[test]
fn assumptions_map_kinds_to_power_states() {
    let a = Assumptions::default();
    assert_eq!(GapKind::Sleep.power_state(&a), PowerState::Sleep);
    assert_eq!(GapKind::Restart.power_state(&a), PowerState::Idle);
    assert_eq!(GapKind::Crash.power_state(&a), PowerState::Off);
    let b = Assumptions { crash: PowerState::Idle, ..a };
    assert_eq!(GapKind::Crash.power_state(&b), PowerState::Idle);
}

// ---------- heartbeat ----------

fn hb(time: DateTime<Utc>, session: &str) -> Heartbeat {
    Heartbeat { time, session_id: session.into(), state: PowerState::Active, source: PowerSource::Ac }
}

#[test]
fn heartbeat_gap_detection() {
    let cfg = HeartbeatConfig::default(); // 30 s, tolerance x3 = 90 s
    let last = hb(t(2026, 5, 1, 12, 0, 0), "a");
    let now = |s| t(2026, 5, 1, 12, 0, 0) + Duration::seconds(s);
    assert!(detect_gap(Some(&last), now(60), "a", &cfg).is_none());
    assert!(detect_gap(Some(&last), now(90), "a", &cfg).is_none());
    assert!(detect_gap(Some(&last), now(91), "a", &cfg).is_some());
    // tracker restarted: even a 45 s silence is checked against the event log
    assert!(detect_gap(Some(&last), now(45), "b", &cfg).is_some());
    assert!(detect_gap(Some(&last), now(20), "b", &cfg).is_none());
    // first run and clock going backwards
    assert!(detect_gap(None, now(1000), "a", &cfg).is_none());
    assert!(detect_gap(Some(&last), now(-5), "a", &cfg).is_none());
}

// ---------- parsing ----------

#[test]
fn xml_parser_handles_empty_and_unwrapped_input() {
    assert!(parse_events_xml("").unwrap().is_empty());
    let one = "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='EventLog'/>\
               <EventID Qualifiers='1'>6005</EventID><TimeCreated SystemTime='2026-05-01T10:00:00.5Z'/></System>\
               <EventData><Binary>00</Binary></EventData></Event>";
    let evs = parse_events_xml(one).unwrap();
    assert_eq!((evs[0].id, evs[0].provider.as_str()), (6005, "EventLog"));
    assert!(parse_events_xml("<Event><oops").is_err());
}

#[cfg(windows)]
#[test]
fn wevtutil_query_mentions_every_provider() {
    let q = gap_classifier::wevtutil::build_query(t(2026, 5, 1, 0, 0, 0), t(2026, 5, 2, 0, 0, 0));
    for p in ["Kernel-Power", "Kernel-General", "Kernel-Boot", "EventLog", "User32", "Power-Troubleshooter"] {
        assert!(q.contains(p), "{p} missing from {q}");
    }
    assert!(q.contains("2026-05-01T00:00:00.000Z"));
}

/// Live check against this machine's real log. Run with: cargo test -p gap-classifier -- --ignored
#[cfg(windows)]
#[test]
#[ignore]
fn live_wevtutil_reads_the_real_log() {
    use gap_classifier::wevtutil::WevtutilSource;
    let now = Utc::now();
    let evs = WevtutilSource.events(now - Duration::days(4), now).expect("wevtutil should work unprivileged");
    println!("{} power events in the last 4 days", evs.len());
}
