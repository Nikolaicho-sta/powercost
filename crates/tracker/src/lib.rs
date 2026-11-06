//! The background tracker: samples sensors, turns them into one priced span per minute,
//! writes heartbeats, and recovers gaps (sleep, shutdown, crash) from the Windows event log.
//!
//! Time is injected (`tick(now)`) so everything is testable; the app drives it with the
//! real clock. Cost of running: a handful of cheap syscalls every `sample_interval`.

use chrono::{DateTime, Duration, Timelike, Utc};
use cost_engine::TariffSet;
use dashboard::CurrentInfo;
use estimator::{Estimator, Inputs};
use gap_classifier::heartbeat::{detect_gap, Heartbeat, HeartbeatConfig, PowerSource};
use gap_classifier::{classify, Assumptions, Config as GapConfig, EventSource, Gap, GapReport};
use pc_core::PowerState;
use sensors::{SensorHub, SensorIssue};
use state_detection::{Observation, StateDetector, Thresholds};
use storage::{Span, SpanSource, Storage};

#[derive(Debug, thiserror::Error)]
pub enum TrackerError {
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
    #[error("event log: {0}")]
    Events(String),
}

pub type Result<T> = std::result::Result<T, TrackerError>;

#[derive(Debug, Clone)]
pub struct TrackerConfig {
    pub sample_interval: Duration,
    pub heartbeat_interval: Duration,
    /// Silence longer than `sample_interval * gap_factor` (and at least 90 s) is a gap.
    pub gap_factor: i32,
    /// How often recent power events are copied into the database (D11).
    pub persist_events_every: Duration,
    /// Assumptions for gaps left over from a previous run (tracker was not running).
    pub assumptions: Assumptions,
    pub thresholds: Thresholds,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            sample_interval: Duration::seconds(10),
            heartbeat_interval: Duration::seconds(30),
            gap_factor: 3,
            persist_events_every: Duration::minutes(30),
            assumptions: Assumptions::default(),
            thresholds: Thresholds::default(),
        }
    }
}

#[derive(Debug, Default)]
pub struct StartupReport {
    pub gap: Option<GapReport>,
    pub spans_added: usize,
    pub events_saved: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub spans_written: usize,
    pub gap_handled: bool,
}

fn whole_secs(t: DateTime<Utc>) -> DateTime<Utc> {
    t.with_nanosecond(0).unwrap()
}

fn minute_of(t: DateTime<Utc>) -> i64 {
    t.timestamp().div_euclid(60)
}

/// Sensor samples of the minute in progress.
#[derive(Debug, Clone)]
struct MinuteAcc {
    minute: i64,
    start: DateTime<Utc>,
    n: u32,
    active: u32,
    ac: u32,
    ac_known: u32,
    cpu_power: (f64, u32),
    gpu_power: (f64, u32),
    cpu_load: f64,
    gpu_util: f64,
    charge: (f64, u32),
}

impl MinuteAcc {
    fn new(minute: i64, start: DateTime<Utc>) -> Self {
        Self { minute, start, n: 0, active: 0, ac: 0, ac_known: 0, cpu_power: (0.0, 0), gpu_power: (0.0, 0), cpu_load: 0.0, gpu_util: 0.0, charge: (0.0, 0) }
    }

    fn avg((sum, n): (f64, u32)) -> Option<f64> {
        (n > 0).then(|| sum / n as f64)
    }

    fn inputs(&self) -> Inputs {
        let n = self.n.max(1) as f64;
        let state = if self.active * 2 >= self.n && self.active > 0 { PowerState::Active } else { PowerState::Idle };
        Inputs {
            state,
            cpu_power_w: Self::avg(self.cpu_power),
            gpu_power_w: Self::avg(self.gpu_power),
            cpu_load_pct: self.cpu_load / n,
            gpu_util_pct: self.gpu_util / n,
            charge_w: Self::avg(self.charge),
            sleep_active_permille: None,
        }
    }

    /// Mains unless most samples say battery; unknown counts as mains.
    fn billable(&self) -> bool {
        self.ac_known == 0 || self.ac * 2 >= self.ac_known
    }
}

pub struct Tracker {
    store: Storage,
    hub: SensorHub,
    detector: StateDetector,
    estimator: Estimator,
    tariffs: TariffSet,
    events: Box<dyn EventSource + Send>,
    cfg: TrackerConfig,
    session_id: String,
    acc: Option<MinuteAcc>,
    covered_until: Option<DateTime<Utc>>,
    last_tick: Option<DateTime<Utc>>,
    last_heartbeat: Option<DateTime<Utc>>,
    last_persist: Option<DateTime<Utc>>,
    last_source: PowerSource,
    last_state: PowerState,
    current: Option<CurrentInfo>,
    issues: Vec<SensorIssue>,
}

impl Tracker {
    pub fn new(
        store: Storage,
        hub: SensorHub,
        estimator: Estimator,
        tariffs: TariffSet,
        events: Box<dyn EventSource + Send>,
        cfg: TrackerConfig,
        session_id: String,
    ) -> Self {
        Self {
            detector: StateDetector::new(cfg.thresholds),
            store,
            hub,
            estimator,
            tariffs,
            events,
            cfg,
            session_id,
            acc: None,
            covered_until: None,
            last_tick: None,
            last_heartbeat: None,
            last_persist: None,
            last_source: PowerSource::Unknown,
            last_state: PowerState::Idle,
            current: None,
            issues: Vec::new(),
        }
    }

    pub fn store(&self) -> &Storage {
        &self.store
    }
    pub fn store_mut(&mut self) -> &mut Storage {
        &mut self.store
    }
    pub fn hub_mut(&mut self) -> &mut SensorHub {
        &mut self.hub
    }
    pub fn current(&self) -> Option<&CurrentInfo> {
        self.current.as_ref()
    }
    pub fn sensor_issues(&self) -> &[SensorIssue] {
        &self.issues
    }
    pub fn set_estimator(&mut self, e: Estimator) {
        self.estimator = e;
    }
    pub fn set_tariffs(&mut self, t: TariffSet) {
        self.tariffs = t;
    }
    pub fn set_thresholds(&mut self, t: Thresholds) {
        self.cfg.thresholds = t;
        self.detector.set_thresholds(t);
    }

    fn gap_threshold(&self) -> Duration {
        (self.cfg.sample_interval * self.cfg.gap_factor).max(Duration::seconds(90))
    }

    /// Call once at start. Recovers the time since the previous run from the event log.
    pub fn startup(&mut self, now: DateTime<Utc>) -> Result<StartupReport> {
        let now = whole_secs(now);
        let mut report = StartupReport::default();
        let last = self.store.last_heartbeat()?;

        // Keep our own copy of recent events before they roll out of the Windows log (D11).
        let from = last.as_ref().map(|h| h.time - Duration::minutes(5)).unwrap_or(now - Duration::days(1));
        report.events_saved = self.persist_events(from, now)?;

        let hb_cfg = HeartbeatConfig { interval: self.cfg.heartbeat_interval, tolerance_factor: self.cfg.gap_factor };
        if let Some(gap) = detect_gap(last.as_ref(), now, &self.session_id, &hb_cfg) {
            let on_ac = last.as_ref().map(|h| h.source != PowerSource::Battery).unwrap_or(true);
            let assumptions = self.cfg.assumptions;
            let rep = self.classify_and_ingest(gap, assumptions, on_ac)?;
            report.spans_added = rep.1;
            report.gap = Some(rep.0);
        }
        self.covered_until = Some(now);
        self.last_persist = Some(now);
        self.write_heartbeat(now)?;
        Ok(report)
    }

    fn persist_events(&mut self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<usize> {
        match self.events.events(from, to) {
            Ok(evs) => Ok(self.store.save_power_events(&evs)?),
            Err(e) => {
                // Not fatal: the log may be unreadable; stored copies still help.
                self.issues.push(SensorIssue { sensor: "event_log", message: e });
                Ok(0)
            }
        }
    }

    fn classify_and_ingest(&mut self, gap: Gap, assumptions: Assumptions, on_ac: bool) -> Result<(GapReport, usize)> {
        let pad = Duration::seconds(120);
        let mut events = self.store.power_events_between(gap.start - pad, gap.end + pad)?;
        if let Ok(live) = self.events.events(gap.start - pad, gap.end + pad) {
            events.extend(live);
        }
        events.sort_by_key(|e| e.time);
        events.dedup_by(|a, b| a.time == b.time && a.provider == b.provider && a.id == b.id);
        let report = classify(gap, &events, &GapConfig::default());
        let res = self.store.ingest_gap(&report, &self.estimator, &assumptions, on_ac, &self.tariffs)?;
        Ok((report, res.spans))
    }

    fn write_heartbeat(&mut self, now: DateTime<Utc>) -> Result<()> {
        self.store.write_heartbeat(&Heartbeat { time: now, session_id: self.session_id.clone(), state: self.last_state, source: self.last_source })?;
        self.last_heartbeat = Some(now);
        Ok(())
    }

    fn flush(&mut self, end: DateTime<Utc>) -> Result<usize> {
        let Some(acc) = self.acc.take() else { return Ok(0) };
        if acc.n == 0 || end <= acc.start {
            return Ok(0);
        }
        let inputs = acc.inputs();
        let est = self.estimator.estimate(&inputs);
        let span = Span {
            start: acc.start,
            end,
            watts: est.watts,
            layer: est.layer,
            billable: acc.billable(),
            source: SpanSource::Live,
            inputs,
        };
        self.store.record_span(&span, &self.tariffs)?;
        self.covered_until = Some(end);
        Ok(1)
    }

    /// One sampling step.
    pub fn tick(&mut self, now: DateTime<Utc>) -> Result<TickReport> {
        let now = whole_secs(now);
        let mut report = TickReport::default();

        // 1) the process was alive but time jumped (sleep/resume, long stall): explain the gap
        if let Some(last) = self.last_tick {
            if now - last > self.gap_threshold() {
                report.spans_written += self.flush(last + self.cfg.sample_interval.min(now - last))?;
                let from = self.covered_until.unwrap_or(last);
                // Process kept running, so an unexplained gap is a stall, not a power-off.
                let assumptions = Assumptions { unknown: PowerState::Idle, crash: PowerState::Idle, ..self.cfg.assumptions };
                let on_ac = self.last_source != PowerSource::Battery;
                if now > from {
                    let (_, n) = self.classify_and_ingest(Gap { start: from, end: now }, assumptions, on_ac)?;
                    report.spans_written += n;
                }
                self.covered_until = Some(now);
                self.acc = None;
                report.gap_handled = true;
            }
        }
        self.last_tick = Some(now);

        // 2) sample
        let (snap, issues) = self.hub.sample();
        self.issues.retain(|i| i.sensor == "event_log");
        self.issues.extend(issues);
        let state = self.detector.update(Observation {
            input_idle_secs: snap.input_idle.map(|d| d.as_secs()),
            cpu_percent: snap.cpu_load_percent.unwrap_or(0.0),
            gpu_percent: snap.gpu.map(|g| g.util_percent),
        });
        let ps = snap.power_source;
        let on_ac = ps.and_then(|p| p.ac_online);
        self.last_source = match on_ac {
            Some(true) => PowerSource::Ac,
            Some(false) => PowerSource::Battery,
            None => PowerSource::Unknown,
        };
        self.last_state = state;
        let charge_w = ps.filter(|p| p.charging && on_ac == Some(true)).and_then(|p| p.battery_rate_mw).filter(|r| *r > 0).map(|r| r as f64 / 1000.0);

        // 3) roll the minute
        let minute = minute_of(now);
        if self.acc.as_ref().is_some_and(|a| a.minute != minute) {
            let minute_end = DateTime::from_timestamp((self.acc.as_ref().unwrap().minute + 1) * 60, 0).unwrap();
            report.spans_written += self.flush(minute_end)?;
        }
        let acc = self.acc.get_or_insert_with(|| MinuteAcc::new(minute, self.covered_until.unwrap_or(now)));
        acc.n += 1;
        if state == PowerState::Active {
            acc.active += 1;
        }
        if let Some(a) = on_ac {
            acc.ac_known += 1;
            if a {
                acc.ac += 1;
            }
        }
        if let Some(w) = snap.cpu_power_w {
            acc.cpu_power.0 += w;
            acc.cpu_power.1 += 1;
        }
        if let Some(g) = snap.gpu {
            acc.gpu_power.0 += g.watts;
            acc.gpu_power.1 += 1;
            acc.gpu_util += g.util_percent;
        }
        acc.cpu_load += snap.cpu_load_percent.unwrap_or(0.0);
        if let Some(c) = charge_w {
            acc.charge.0 += c;
            acc.charge.1 += 1;
        }

        // 4) instantaneous estimate for the tray/dashboard
        let inst = Inputs {
            state,
            cpu_power_w: snap.cpu_power_w,
            gpu_power_w: snap.gpu.map(|g| g.watts),
            cpu_load_pct: snap.cpu_load_percent.unwrap_or(0.0),
            gpu_util_pct: snap.gpu.map(|g| g.util_percent).unwrap_or(0.0),
            charge_w,
            sleep_active_permille: None,
        };
        self.current = Some(CurrentInfo::new(state, &self.estimator.estimate(&inst), on_ac));

        // 5) housekeeping
        if self.last_heartbeat.is_none_or(|h| now - h >= self.cfg.heartbeat_interval) {
            self.write_heartbeat(now)?;
        }
        if self.last_persist.is_none_or(|p| now - p >= self.cfg.persist_events_every) {
            let from = self.last_persist.unwrap_or(now - Duration::hours(1)) - Duration::minutes(5);
            self.persist_events(from, now)?;
            self.last_persist = Some(now);
        }
        Ok(report)
    }

    /// Clean stop: store the minute in progress and a final heartbeat.
    pub fn shutdown(&mut self, now: DateTime<Utc>) -> Result<()> {
        let now = whole_secs(now);
        self.flush(now)?;
        self.write_heartbeat(now)?;
        Ok(())
    }
}
