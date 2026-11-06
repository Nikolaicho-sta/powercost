//! Stopwatch: "start the clock, use the PC, stop the clock, see what it cost".
//!
//! The result is exact to the second: stored spans are clipped to the interval and priced with the
//! tariff versions that applied. The minute still in progress is not stored yet, so the tail after
//! the last stored span is filled with the tracker's current estimate, and the result says how many
//! seconds were estimated that way and how many had no data at all.

use crate::BackendError;
use chrono::{DateTime, Utc};
use cost_engine::{nano_to_minor, price_samples, summarize, TariffSet, UsageSample};
use pc_core::{PowerState, NANO};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use storage::Storage;
use tariff_engine::Tariff;

const RUNNING_KEY: &str = "stopwatch_running";
const HISTORY_KEY: &str = "stopwatch_history";
const HISTORY_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub start: DateTime<Utc>,
    pub label: String,
}

/// The tracker's current instantaneous estimate, used for the not-yet-stored last minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tail {
    pub watts: Decimal,
    pub state: PowerState,
    /// False while running on battery.
    pub billable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateLine {
    pub state: PowerState,
    pub seconds: i64,
    pub kwh_nano: i64,
    pub cost_nano: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodLine {
    pub id: String,
    pub label: String,
    pub seconds: i64,
    pub kwh_nano: i64,
    pub cost_nano: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub seconds: i64,
    pub kwh_nano: i64,
    pub cost_nano: i64,
    /// Cost rounded half-even to minor units (cents).
    pub cost_minor: i64,
    pub currency: String,
    pub minor_digits: u32,
    /// Average wall power over the priced time, one decimal ("42.5").
    pub avg_watts: String,
    pub by_state: Vec<StateLine>,
    pub by_period: Vec<PeriodLine>,
    /// Seconds filled from the live estimate (the last minute was not stored yet).
    pub estimated_tail_seconds: i64,
    /// Seconds in the interval for which the tracker had no data (app not running).
    pub untracked_seconds: i64,
    pub measured_sensors_permille: u32,
    pub calibrated_permille: u32,
    pub default_model_permille: u32,
    pub tariff_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub label: String,
    pub result: Measurement,
}

fn digits(currency: &str) -> u32 {
    if matches!(currency, "JPY" | "KRW" | "HUF") {
        0
    } else {
        2
    }
}

/// Energy and cost of `[start, end)`.
pub fn measure(store: &Storage, set: &TariffSet, tariff: &Tariff, start: DateTime<Utc>, end: DateTime<Utc>, tail: Option<Tail>) -> Result<Measurement, BackendError> {
    let total_secs = (end - start).num_seconds().max(0);
    let mut samples: Vec<UsageSample> = Vec::new();
    let mut covered_until = start;
    let mut layer_secs: std::collections::BTreeMap<&'static str, i64> = std::collections::BTreeMap::new();
    let mut covered_secs = 0i64;

    if end > start {
        for s in store.spans_between(start, end)? {
            let (a, b) = (s.start.max(start), s.end.min(end));
            if b <= a {
                continue;
            }
            let secs = (b - a).num_seconds();
            covered_secs += secs;
            let layer = match s.layer {
                estimator::Layer::Sensors => "s",
                estimator::Layer::Calibrated => "c",
                estimator::Layer::Default => "d",
            };
            if matches!(s.inputs.state, PowerState::Active | PowerState::Idle | PowerState::Sleep) {
                *layer_secs.entry(layer).or_default() += secs;
            }
            samples.push(UsageSample { start: a, end: b, state: s.inputs.state, watts: s.watts, billable: s.billable });
            covered_until = covered_until.max(b);
        }
    }

    let mut tail_secs = 0;
    if let Some(t) = tail {
        if end > covered_until {
            tail_secs = (end - covered_until).num_seconds();
            samples.push(UsageSample { start: covered_until, end, state: t.state, watts: t.watts, billable: t.billable });
        }
    }

    let intervals = price_samples(set, &samples)?;
    let summary = summarize(&intervals);
    let d = digits(&tariff.currency);

    let by_state = PowerState::ALL
        .iter()
        .filter_map(|s| summary.by_state.get(s).map(|t| StateLine { state: *s, seconds: t.seconds, kwh_nano: t.kwh_nano, cost_nano: t.cost_nano }))
        .collect();
    let by_period = tariff
        .periods
        .iter()
        .filter_map(|p| summary.by_period.get(&p.id).map(|t| PeriodLine { id: p.id.clone(), label: p.label.clone(), seconds: t.seconds, kwh_nano: t.kwh_nano, cost_nano: t.cost_nano }))
        .collect();

    let priced_secs = summary.total.seconds;
    let avg = if priced_secs > 0 {
        (Decimal::from(summary.total.kwh_nano) / Decimal::from(NANO) * Decimal::from(3_600_000) / Decimal::from(priced_secs)).round_dp(1)
    } else {
        Decimal::ZERO
    };
    let mut avg_text = avg.to_string();
    if !avg_text.contains('.') {
        avg_text.push_str(".0");
    }

    let layer_total: i64 = layer_secs.values().sum();
    let pm = |k: &str| if layer_total > 0 { (layer_secs.get(k).copied().unwrap_or(0) * 1000 / layer_total) as u32 } else { 0 };
    let (s, c, df) = (pm("s"), pm("c"), pm("d"));
    // make the three add up to 1000 when there is any data (remainder to the largest)
    let mut mix = [s, c, df];
    if layer_total > 0 {
        let sum: u32 = mix.iter().sum();
        let idx = (0..3).max_by_key(|&i| mix[i]).unwrap();
        mix[idx] += 1000 - sum;
    }

    Ok(Measurement {
        start,
        end,
        seconds: total_secs,
        kwh_nano: summary.total.kwh_nano,
        cost_nano: summary.total.cost_nano,
        cost_minor: nano_to_minor(summary.total.cost_nano, d),
        currency: tariff.currency.clone(),
        minor_digits: d,
        avg_watts: avg_text,
        by_state,
        by_period,
        estimated_tail_seconds: tail_secs,
        untracked_seconds: (total_secs - covered_secs - tail_secs).max(0),
        measured_sensors_permille: mix[0],
        calibrated_permille: mix[1],
        default_model_permille: mix[2],
        tariff_name: tariff.name.clone(),
    })
}

// ---------------------------------------------------------------- state

pub fn running(store: &Storage) -> Result<Option<Running>, BackendError> {
    // `cancel` stores an explicit null, so read it as an Option.
    Ok(store.get_setting::<Option<Running>>(RUNNING_KEY)?.flatten())
}

pub fn start(store: &Storage, label: &str, now: DateTime<Utc>) -> Result<Running, BackendError> {
    if running(store)?.is_some() {
        return Err(BackendError::Message("the stopwatch is already running".into()));
    }
    let r = Running { start: now, label: label.trim().chars().take(80).collect() };
    store.set_setting(RUNNING_KEY, &r)?;
    Ok(r)
}

/// Drop the running stopwatch without saving anything.
pub fn cancel(store: &Storage) -> Result<(), BackendError> {
    store.set_setting::<Option<Running>>(RUNNING_KEY, &None)?;
    Ok(())
}

pub fn history(store: &Storage) -> Result<Vec<Session>, BackendError> {
    Ok(store.get_setting::<Vec<Session>>(HISTORY_KEY)?.unwrap_or_default())
}

pub fn stop(store: &Storage, set: &TariffSet, tariff: &Tariff, tail: Option<Tail>, now: DateTime<Utc>) -> Result<Session, BackendError> {
    let Some(r) = running(store)? else {
        return Err(BackendError::Message("the stopwatch is not running".into()));
    };
    let result = measure(store, set, tariff, r.start, now.max(r.start), tail)?;
    let session = Session { id: format!("{}", r.start.timestamp_millis()), label: r.label, result };
    let mut h = history(store)?;
    h.insert(0, session.clone());
    h.truncate(HISTORY_LIMIT);
    store.set_setting(HISTORY_KEY, &h)?;
    cancel(store)?;
    Ok(session)
}

pub fn delete(store: &Storage, id: &str) -> Result<(), BackendError> {
    let mut h = history(store)?;
    h.retain(|s| s.id != id);
    store.set_setting(HISTORY_KEY, &h)?;
    Ok(())
}
