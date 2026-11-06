//! Gap classifier: given the time span in which the tracker was NOT running
//! (last heartbeat -> now) and the Windows power events from that span, split the span
//! into segments (sleep / hibernate / shutdown / restart / crash / powered-on-untracked).
//!
//! What was learned from the reference laptop (Windows 11, Modern Standby):
//! * Sleep is Modern Standby: Kernel-Power 506 (enter) / 507 (exit). Classic 42/107 never occur.
//!   507 carries DurationInUs, ActiveResidencyInUs (non-DRIPS time) and PowerStateAc, so a
//!   "sleep" can be mostly *active* (lid closed + external monitor + audio). The segment
//!   keeps these figures so the estimator can price the active part properly.
//! * Restart: User32 1074 (intent) -> EventLog 6006 -> Kernel-Power 109 -> Kernel-General 13,
//!   then Kernel-General 12 / Kernel-Boot 20,27 / EventLog 6005 at the next boot.
//! * Kernel-Boot 20 has LastShutdownGood; Kernel-Boot 27 has BootType (0 full, 1 fast startup,
//!   2 resume from hibernate - 1 and 2 are documented, not yet observed here).
//! * NOT observed on the reference machine (fixtures are synthetic, built from documented
//!   semantics): crash (Kernel-Power 41 / EventLog 6008 / LastShutdownGood=false),
//!   hibernate (Kernel-Power 42 with TargetState 4), classic S3 sleep (42 / 107).
//! * The System log on that machine holds only ~5 days, so the app must persist these
//!   events itself; this crate only interprets what it is given.

pub mod event;
pub mod heartbeat;
#[cfg(windows)]
pub mod wevtutil;

pub use event::{parse_events_xml, PowerEvent};

use chrono::{DateTime, Duration, Utc};
use pc_core::PowerState;
use serde::{Deserialize, Serialize};

/// Where events come from (real: wevtutil; tests: fixtures).
pub trait EventSource {
    fn events(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<PowerEvent>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// PC was on (evidence says so) but the tracker was not recording.
    OnUntracked,
    Sleep,
    Hibernate,
    Shutdown,
    /// Reboot: powered, but not recording.
    Restart,
    /// Power loss / BSOD / forced off: start of the outage is unknown.
    Crash,
    /// No evidence at all (event log rolled over, or no events in the span).
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub provider: String,
    pub id: u32,
    pub time: DateTime<Utc>,
}

/// Energy-relevant facts from Modern Standby exit events (Kernel-Power 507).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepDetails {
    pub duration_us: Option<u64>,
    /// Time the system was NOT in the low-power state (CPU running) during the standby.
    pub active_us: Option<u64>,
    pub on_ac: Option<bool>,
    /// Battery energy at exit minus at entry, mWh (negative = drained).
    pub battery_delta_mwh: Option<i64>,
}

impl SleepDetails {
    /// Share of the sleep spent actively running, in per-mille (0..=1000).
    pub fn active_permille(&self) -> Option<u32> {
        match (self.active_us, self.duration_us) {
            (Some(a), Some(d)) if d > 0 => Some(((a.min(d) as u128 * 1000) / d as u128) as u32),
            _ => None,
        }
    }

    fn merge(self, o: SleepDetails) -> SleepDetails {
        fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            match (a, b) {
                (Some(x), Some(y)) => Some(x + y),
                (x, None) | (None, x) => x,
            }
        }
        SleepDetails {
            duration_us: add(self.duration_us, o.duration_us),
            active_us: add(self.active_us, o.active_us),
            on_ac: match (self.on_ac, o.on_ac) {
                (Some(a), Some(b)) if a == b => Some(a),
                (Some(_), Some(_)) => None,
                (x, None) | (None, x) => x,
            },
            battery_delta_mwh: match (self.battery_delta_mwh, o.battery_delta_mwh) {
                (Some(a), Some(b)) => Some(a + b),
                (x, None) | (None, x) => x,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub kind: GapKind,
    pub confidence: Confidence,
    pub evidence: Vec<EvidenceRef>,
    pub sleep: Option<SleepDetails>,
    pub note: Option<String>,
}

impl Segment {
    pub fn seconds(&self) -> i64 {
        (self.end - self.start).num_seconds()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapReport {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Events up to this long before the gap start still belong to it (heartbeat granularity).
    pub lead_tolerance: Duration,
    /// Powered-on blips shorter than this between two sleeps are folded into the sleep.
    pub flicker: Duration,
    /// How long after a boot marker, crash evidence is still attributed to that boot.
    pub boot_window: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self { lead_tolerance: Duration::seconds(120), flicker: Duration::seconds(10), boot_window: Duration::seconds(180) }
    }
}

/// User-configurable assumptions turning an uncertain segment into a power state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assumptions {
    pub crash: PowerState,
    pub unknown: PowerState,
    pub untracked_on: PowerState,
}

impl Default for Assumptions {
    fn default() -> Self {
        Self { crash: PowerState::Off, unknown: PowerState::Off, untracked_on: PowerState::Idle }
    }
}

impl GapKind {
    pub fn power_state(self, a: &Assumptions) -> PowerState {
        match self {
            GapKind::OnUntracked | GapKind::Restart => a.untracked_on,
            GapKind::Sleep => PowerState::Sleep,
            GapKind::Hibernate => PowerState::Hibernate,
            GapKind::Shutdown => PowerState::Off,
            GapKind::Crash => a.crash,
            GapKind::Unknown => a.unknown,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Event interpretation
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Signal {
    SleepEnter { hibernate: bool },
    SleepExit(SleepDetails),
    /// 1074: says whether a shutdown or a restart was requested. Does not change the state.
    ShutdownIntent { restart: bool },
    /// 6006 / 109 / 13: OS going down.
    ShutdownBegin,
    OsStart,
    BootType(u8),
    CrashEvidence,
}

fn interpret(e: &PowerEvent) -> Option<Signal> {
    match (e.short_provider(), e.id) {
        ("Kernel-Power", 506) => Some(Signal::SleepEnter { hibernate: false }),
        // Classic suspend. TargetState 4 = hibernate (documented; not observed on reference HW).
        ("Kernel-Power", 42) => Some(Signal::SleepEnter { hibernate: e.get("TargetState") == Some("4") }),
        ("Kernel-Power", 507) => Some(Signal::SleepExit(SleepDetails {
            duration_us: e.get_u64("DurationInUs"),
            active_us: e.get_u64("ActiveResidencyInUs"),
            on_ac: e.get("PowerStateAc").map(|v| v == "true"),
            battery_delta_mwh: match (e.get_i64("SleepEnergyCapacityAtStart"), e.get_i64("SleepEnergyCapacityAtEnd")) {
                (Some(s), Some(en)) => Some(en - s),
                _ => None,
            },
        })),
        ("Kernel-Power", 107) | ("Power-Troubleshooter", 1) => Some(Signal::SleepExit(SleepDetails::default())),
        ("User32", 1074) => {
            let what = e.get("param5").unwrap_or("").to_ascii_lowercase();
            Some(Signal::ShutdownIntent { restart: what.contains("restart") || what.contains("reboot") })
        }
        ("EventLog", 6006) | ("Kernel-Power", 109) | ("Kernel-General", 13) => Some(Signal::ShutdownBegin),
        ("EventLog", 6005) | ("Kernel-General", 12) => Some(Signal::OsStart),
        ("Kernel-Boot", 27) => e.get_u64("BootType").map(|b| Signal::BootType(b as u8)),
        ("Kernel-Power", 41) | ("EventLog", 6008) => Some(Signal::CrashEvidence),
        ("Kernel-Boot", 20) if e.get("LastShutdownGood") == Some("false") => Some(Signal::CrashEvidence),
        _ => None,
    }
}

fn evref(e: &PowerEvent) -> EvidenceRef {
    EvidenceRef { provider: e.provider.clone(), id: e.id, time: e.time }
}

#[derive(Debug)]
enum State {
    On,
    Sleeping { since: DateTime<Utc>, hibernate: bool, ev: Vec<EvidenceRef> },
    Off { since: DateTime<Utc>, restart: bool, ev: Vec<EvidenceRef> },
}

fn seg(start: DateTime<Utc>, end: DateTime<Utc>, kind: GapKind, confidence: Confidence, ev: Vec<EvidenceRef>) -> Segment {
    Segment { start, end, kind, confidence, evidence: ev, sleep: None, note: None }
}

/// Classify one gap.
pub fn classify(gap: Gap, events: &[PowerEvent], cfg: &Config) -> GapReport {
    let lo = gap.start - cfg.lead_tolerance;
    let mut evs: Vec<&PowerEvent> = events
        .iter()
        .filter(|e| e.time >= lo && e.time <= gap.end + cfg.lead_tolerance && interpret(e).is_some())
        .collect();
    evs.sort_by_key(|e| e.time);
    let clamp = |t: DateTime<Utc>| t.clamp(gap.start, gap.end);

    // Boot facts attributed to the OsStart they follow closely.
    let boot_type_near = |t: DateTime<Utc>| {
        evs.iter().find_map(|e| match interpret(e) {
            Some(Signal::BootType(b)) if e.time >= t - Duration::seconds(5) && e.time <= t + cfg.boot_window => Some(b),
            _ => None,
        })
    };
    let crash_near = |t: DateTime<Utc>| -> Vec<EvidenceRef> {
        evs.iter()
            .filter(|e| {
                matches!(interpret(e), Some(Signal::CrashEvidence))
                    && e.time >= t - Duration::seconds(5)
                    && e.time <= t + cfg.boot_window
            })
            .map(|e| evref(e))
            .collect()
    };

    let mut segs: Vec<Segment> = Vec::new();
    let mut state = State::On;
    let mut pending_restart: Option<DateTime<Utc>> = None;
    // Time of the last boot marker handled; later markers of the same boot are ignored.
    let mut last_boot: Option<DateTime<Utc>> = None;
    // Latest instant up to which `segs` explains the gap; used as crash start when needed.
    let mut covered_to = gap.start;

    for e in &evs {
        let t = clamp(e.time);
        match interpret(e).expect("filtered") {
            Signal::SleepEnter { hibernate } => {
                if matches!(state, State::On) {
                    state = State::Sleeping { since: t, hibernate, ev: vec![evref(e)] };
                }
            }
            Signal::SleepExit(details) => match std::mem::replace(&mut state, State::On) {
                State::Sleeping { since, hibernate, mut ev } => {
                    ev.push(evref(e));
                    let mut s = seg(
                        since,
                        t,
                        if hibernate { GapKind::Hibernate } else { GapKind::Sleep },
                        Confidence::High,
                        ev,
                    );
                    s.sleep = Some(details);
                    covered_to = t;
                    segs.push(s);
                }
                State::On => {
                    // Exit without a seen enter: the enter happened before the gap/ log window.
                    let start = match details.duration_us {
                        Some(us) => clamp(e.time - Duration::microseconds(us as i64)),
                        None => gap.start,
                    };
                    let mut s = seg(start, t, GapKind::Sleep, Confidence::Medium, vec![evref(e)]);
                    // If the sleep began before the gap, keep only the part inside it.
                    let mut details = details;
                    if let Some(full) = details.duration_us.filter(|&d| d > 0) {
                        let inside = (t - start).num_microseconds().unwrap_or(0).max(0) as u64;
                        if inside < full {
                            details.active_us = details.active_us.map(|a| (a as u128 * inside as u128 / full as u128) as u64);
                            details.battery_delta_mwh = None; // cannot split a battery delta
                            details.duration_us = Some(inside);
                        }
                    }
                    s.sleep = Some(details);
                    s.note = Some("sleep entry not in event window; start inferred".into());
                    covered_to = t;
                    segs.push(s);
                }
                other => state = other,
            },
            Signal::ShutdownIntent { restart } => {
                if restart {
                    pending_restart = Some(e.time);
                }
            }
            Signal::ShutdownBegin => {
                if matches!(state, State::Sleeping { .. }) {
                    if let State::Sleeping { since, hibernate, ev } = std::mem::replace(&mut state, State::On) {
                        let kind = if hibernate { GapKind::Hibernate } else { GapKind::Sleep };
                        segs.push(seg(since, t, kind, Confidence::Medium, ev));
                        covered_to = t;
                    }
                }
                if !matches!(state, State::Off { .. }) {
                    let restart = pending_restart.is_some_and(|p| e.time - p < Duration::minutes(10));
                    state = State::Off { since: t, restart, ev: vec![evref(e)] };
                } else if let State::Off { ev, .. } = &mut state {
                    ev.push(evref(e));
                }
            }
            Signal::BootType(_) | Signal::CrashEvidence => {}
            Signal::OsStart => match std::mem::replace(&mut state, State::On) {
                State::Off { since, restart, mut ev } => {
                    last_boot = Some(e.time);
                    ev.push(evref(e));
                    let bt = boot_type_near(e.time);
                    let (kind, conf) = if bt == Some(2) {
                        (GapKind::Hibernate, Confidence::Medium)
                    } else if restart {
                        (GapKind::Restart, Confidence::High)
                    } else {
                        (GapKind::Shutdown, Confidence::High)
                    };
                    let mut s = seg(since, t, kind, conf, ev);
                    if bt == Some(1) {
                        s.note = Some("Fast Startup: shutdown is a hybrid hibernate, power draw is standby level".into());
                    }
                    covered_to = t;
                    segs.push(s);
                    pending_restart = None;
                }
                State::Sleeping { since, hibernate, mut ev } => {
                    last_boot = Some(e.time);
                    ev.push(evref(e));
                    let bt = boot_type_near(e.time);
                    if hibernate || bt == Some(2) {
                        covered_to = t;
                        segs.push(seg(since, t, GapKind::Hibernate, Confidence::Medium, ev));
                    } else {
                        // Slept, then booted fresh: power was lost while asleep.
                        covered_to = t;
                        let mut s = seg(since, t, GapKind::Crash, Confidence::Medium, ev);
                        s.note = Some("slept, then cold boot: power loss/battery empty during sleep".into());
                        segs.push(s);
                    }
                }
                State::On if last_boot.is_some_and(|b| e.time - b <= cfg.boot_window) => {
                    // Second marker of the same boot (Kernel-General 12, then EventLog 6005 ~30 s later).
                }
                State::On => {
                    last_boot = Some(e.time);
                    // Booted without any shutdown: crash / power loss since the last known-on moment.
                    let marks = crash_near(e.time);
                    let conf = if marks.is_empty() { Confidence::Medium } else { Confidence::High };
                    let mut ev = marks;
                    ev.push(evref(e));
                    let mut s = seg(covered_to, t, GapKind::Crash, conf, ev);
                    s.note = Some("no shutdown events before boot; outage start unknown (between last heartbeat and boot)".into());
                    segs.push(s);
                    covered_to = t;
                }
            },
        }
    }

    // Close open state at the end of the gap.
    match state {
        State::Sleeping { since, hibernate, ev } => {
            let kind = if hibernate { GapKind::Hibernate } else { GapKind::Sleep };
            let mut s = seg(since, gap.end, kind, Confidence::Low, ev);
            s.note = Some("no wake event seen; assumed asleep until tracker resumed".into());
            segs.push(s);
        }
        State::Off { since, restart, ev } => {
            let kind = if restart { GapKind::Restart } else { GapKind::Shutdown };
            let mut s = seg(since, gap.end, kind, Confidence::Low, ev);
            s.note = Some("no boot event seen; assumed off until tracker resumed".into());
            segs.push(s);
        }
        State::On => {}
    }

    segs.retain(|s| s.end > s.start);
    segs.sort_by_key(|s| s.start);

    // Fill uncovered parts: powered on but untracked (or Unknown if nothing was seen).
    let evidence_found = !segs.is_empty();
    let fill_kind = if evidence_found { GapKind::OnUntracked } else { GapKind::Unknown };
    let fill_conf = if evidence_found { Confidence::Medium } else { Confidence::Low };
    let mut filled = Vec::new();
    let mut cursor = gap.start;
    for s in segs {
        if s.start > cursor {
            let mut f = seg(cursor, s.start, fill_kind, fill_conf, vec![]);
            if !evidence_found {
                f.note = Some("no power events found for this span (log rolled over?)".into());
            }
            filled.push(f);
        }
        cursor = cursor.max(s.end);
        filled.push(s);
    }
    if cursor < gap.end {
        let mut f = seg(cursor, gap.end, fill_kind, fill_conf, vec![]);
        if !evidence_found {
            f.note = Some("no power events found for this span (log rolled over?)".into());
        }
        filled.push(f);
    }

    GapReport { start: gap.start, end: gap.end, segments: merge_flicker(filled, cfg) }
}

/// Fold "sleep, tiny powered-on blip, sleep" into one sleep, and merge touching equal-kind sleeps.
fn merge_flicker(mut segs: Vec<Segment>, cfg: &Config) -> Vec<Segment> {
    loop {
        let mut changed = false;
        // drop short OnUntracked blips between two sleeps
        let mut i = 1;
        while i + 1 < segs.len() {
            if segs[i].kind == GapKind::OnUntracked
                && segs[i - 1].kind == GapKind::Sleep
                && segs[i + 1].kind == GapKind::Sleep
                && segs[i].end - segs[i].start <= cfg.flicker
            {
                segs.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
        // merge adjacent sleeps
        let mut i = 0;
        while i + 1 < segs.len() {
            if segs[i].kind == GapKind::Sleep && segs[i + 1].kind == GapKind::Sleep {
                let b = segs.remove(i + 1);
                let a = &mut segs[i];
                a.end = b.end;
                a.evidence.extend(b.evidence);
                a.confidence = a.confidence.min(b.confidence);
                a.sleep = match (a.sleep, b.sleep) {
                    (Some(x), Some(y)) => Some(x.merge(y)),
                    (x, None) | (None, x) => x,
                };
                a.note = a.note.take().or(b.note);
                changed = true;
            } else {
                i += 1;
            }
        }
        if !changed {
            return segs;
        }
    }
}

/// Query `source` for the gap (padded) and classify it.
pub fn classify_with_source(gap: Gap, source: &dyn EventSource, cfg: &Config) -> Result<GapReport, String> {
    let events = source.events(gap.start - cfg.lead_tolerance, gap.end + cfg.lead_tolerance)?;
    Ok(classify(gap, &events, cfg))
}

