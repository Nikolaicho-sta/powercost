//! Cost engine: turns usage spans (UTC, state, watts) into priced 1-minute intervals.
//!
//! * Spans are cut at UTC minute boundaries. All real-world UTC offsets are whole minutes,
//!   so tariff boundaries (whole local minutes) never fall inside a cut segment.
//! * Energy is kept as exact `Decimal` while pricing and only rounded (half-even) to
//!   nano-units (1e-9) when stored in `Interval`; aggregation sums integers, so totals are
//!   exact and reproducible. Rounding to cents happens only at display time.
//! * Non-billable usage (laptop on battery) keeps its kWh but costs nothing.
//! * Pricing is a pure function of (raw samples, tariffs): re-deriving after a tariff or
//!   calibration correction is just calling `price_samples` again.

use chrono::{DateTime, Duration, DurationRound, NaiveDate, Utc};
use pc_core::{PowerState, NANO};
use rust_decimal::prelude::*;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tariff_engine::Tariff;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CostError {
    #[error("sample end must be after start")]
    EmptySpan,
    #[error("watts must not be negative")]
    NegativeWatts,
    #[error("no tariff available")]
    NoTariff,
    #[error("amount does not fit in fixed-point storage")]
    Overflow,
}

/// One span of constant power draw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSample {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub state: PowerState,
    /// Estimated wall power in watts.
    pub watts: Decimal,
    /// False when the energy is not billed (e.g. running on battery).
    pub billable: bool,
}

/// A priced slice, at most one UTC minute long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub minute_start: DateTime<Utc>,
    pub seconds: u32,
    pub state: PowerState,
    pub billable: bool,
    /// Energy in nano-kWh (kWh * 1e9).
    pub kwh_nano: i64,
    pub tariff_id: String,
    pub tariff_version: u32,
    pub period_id: String,
    /// Gross price per kWh in nano-units, as used.
    pub price_nano_per_kwh: i64,
    /// Cost in nano-units of currency (0 if not billable).
    pub cost_nano: i64,
}

/// Several tariff versions over time (prices change, e.g. every July 1).
#[derive(Debug, Clone)]
pub struct TariffSet {
    tariffs: Vec<Tariff>,
}

impl TariffSet {
    pub fn new(tariffs: Vec<Tariff>) -> Result<Self, CostError> {
        if tariffs.is_empty() {
            return Err(CostError::NoTariff);
        }
        Ok(Self { tariffs })
    }

    pub fn single(t: Tariff) -> Self {
        Self { tariffs: vec![t] }
    }

    /// Tariff in force on the local date of `utc`: the one with the latest `valid_from`
    /// that is <= the date. If a tariff has expired and nothing replaced it, it keeps being
    /// used (best available estimate; the UI warns). Before the earliest `valid_from`,
    /// the earliest tariff is used.
    pub fn at(&self, utc: DateTime<Utc>) -> &Tariff {
        let mut best: Option<&Tariff> = None;
        for t in &self.tariffs {
            let date: NaiveDate = utc.with_timezone(&t.tz()).date_naive();
            if t.valid_from <= date && best.is_none_or(|b| t.valid_from >= b.valid_from) {
                best = Some(t);
            }
        }
        best.unwrap_or_else(|| self.tariffs.iter().min_by_key(|t| t.valid_from).expect("non-empty"))
    }
}

fn to_nano(value: Decimal) -> Result<i64, CostError> {
    (value * Decimal::from(NANO))
        .round_dp_with_strategy(0, RoundingStrategy::MidpointNearestEven)
        .to_i64()
        .ok_or(CostError::Overflow)
}

/// Price one sample, cutting it into UTC-minute intervals.
pub fn price_sample(tariffs: &TariffSet, s: &UsageSample) -> Result<Vec<Interval>, CostError> {
    if s.end <= s.start {
        return Err(CostError::EmptySpan);
    }
    if s.watts < Decimal::ZERO {
        return Err(CostError::NegativeWatts);
    }
    let mut out = Vec::new();
    let mut cursor = s.start;
    while cursor < s.end {
        let minute_start = cursor.duration_trunc(Duration::minutes(1)).expect("minute truncation");
        let next = (minute_start + Duration::minutes(1)).min(s.end);
        let seconds = (next - cursor).num_seconds();

        // kWh = W * s / 3_600_000, exact in Decimal for integer seconds.
        let kwh = s.watts * Decimal::from(seconds) / Decimal::from(3_600_000);

        let tariff = tariffs.at(minute_start);
        let resolved = tariff.resolve(minute_start);
        let price = tariff.price_for_index(resolved.period_index).gross;

        let cost_nano = if s.billable { to_nano(kwh * price)? } else { 0 };
        out.push(Interval {
            minute_start,
            seconds: seconds as u32,
            state: s.state,
            billable: s.billable,
            kwh_nano: to_nano(kwh)?,
            tariff_id: tariff.id.clone(),
            tariff_version: tariff.version,
            period_id: resolved.period_id.to_string(),
            price_nano_per_kwh: to_nano(price)?,
            cost_nano,
        });
        cursor = next;
    }
    Ok(out)
}

/// Price many samples. Also the "recompute history" entry point: call again with
/// corrected tariffs (or samples re-estimated with a corrected calibration).
pub fn price_samples(tariffs: &TariffSet, samples: &[UsageSample]) -> Result<Vec<Interval>, CostError> {
    let mut all = Vec::new();
    for s in samples {
        all.extend(price_sample(tariffs, s)?);
    }
    Ok(all)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    pub seconds: i64,
    pub kwh_nano: i64,
    pub cost_nano: i64,
}

impl Totals {
    fn add(&mut self, i: &Interval) {
        self.seconds += i.seconds as i64;
        self.kwh_nano += i.kwh_nano;
        self.cost_nano += i.cost_nano;
    }

    /// Cost rounded half-even to minor units (`decimals` = 2 for EUR cents).
    pub fn cost_minor(&self, decimals: u32) -> i64 {
        nano_to_minor(self.cost_nano, decimals)
    }

    pub fn kwh(&self) -> Decimal {
        Decimal::from(self.kwh_nano) / Decimal::from(NANO)
    }
}

pub fn nano_to_minor(nano: i64, decimals: u32) -> i64 {
    let value = Decimal::from(nano) / Decimal::from(NANO) * Decimal::from(10i64.pow(decimals));
    value.round_dp_with_strategy(0, RoundingStrategy::MidpointNearestEven).to_i64().unwrap_or(i64::MAX)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub total: Totals,
    pub by_state: BTreeMap<PowerState, Totals>,
    pub by_period: BTreeMap<String, Totals>,
}

pub fn summarize(intervals: &[Interval]) -> Summary {
    let mut s = Summary::default();
    for i in intervals {
        s.total.add(i);
        s.by_state.entry(i.state).or_default().add(i);
        s.by_period.entry(i.period_id.clone()).or_default().add(i);
    }
    s
}
