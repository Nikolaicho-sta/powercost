//! Dashboard read model: everything the UI shows, computed here (in Rust, with integer
//! money) so the frontend never does money arithmetic. Amounts are raw integers
//! (`kwh_nano`, `cost_nano`, `seconds`); the UI only divides by 1e9 for display.

mod export;
mod whatif;

pub use export::{export, ExportFormat, Granularity};
pub use whatif::{what_if, Scenario, WhatIfParams, WhatIfResult};

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::Totals;
use estimator::Estimate;
use pc_core::PowerState;
use serde::{Deserialize, Serialize};
use storage::{Bucket, Storage};
use tariff_engine::{Tariff, Warning};

#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeriodKind {
    Today,
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub kind: PeriodKind,
    /// Start of the period (local midnight, as UTC).
    pub from: DateTime<Utc>,
    /// `now`, clamped to the period end.
    pub to: DateTime<Utc>,
    /// End of the whole period (for padding the chart with empty future buckets).
    pub period_end: DateTime<Utc>,
}

fn local_midnight(date: NaiveDate, tz: Tz) -> DateTime<Utc> {
    let mut naive = date.and_hms_opt(0, 0, 0).unwrap();
    loop {
        if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
            return dt.with_timezone(&Utc);
        }
        naive += Duration::minutes(30);
    }
}

/// Today / this week (Monday start) / this month / this year, in local time.
pub fn range_for(kind: PeriodKind, now: DateTime<Utc>, tz: Tz) -> Range {
    let today = now.with_timezone(&tz).date_naive();
    let (start, end) = match kind {
        PeriodKind::Today => (today, today.succ_opt().unwrap()),
        PeriodKind::Week => {
            let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
            (monday, monday + Duration::days(7))
        }
        PeriodKind::Month => {
            let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
            let next = if today.month() == 12 {
                NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1).unwrap()
            };
            (first, next)
        }
        PeriodKind::Year => (NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap(), NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).unwrap()),
    };
    let (from, period_end) = (local_midnight(start, tz), local_midnight(end, tz));
    Range { kind, from, to: now.min(period_end), period_end }
}

fn bucket_for(kind: PeriodKind) -> Bucket {
    match kind {
        PeriodKind::Today => Bucket::Hour,
        PeriodKind::Week | PeriodKind::Month => Bucket::Day,
        PeriodKind::Year => Bucket::Month,
    }
}

// ---------------------------------------------------------------- DTOs

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount {
    pub seconds: i64,
    pub kwh_nano: i64,
    pub cost_nano: i64,
    /// Cost rounded half-even to minor units (cents) - computed here so the UI never rounds money.
    pub cost_minor: i64,
}

impl Amount {
    fn with_digits(mut self, digits: u32) -> Self {
        self.cost_minor = cost_engine::nano_to_minor(self.cost_nano, digits);
        self
    }
}

impl From<Totals> for Amount {
    fn from(t: Totals) -> Self {
        Amount { seconds: t.seconds, kwh_nano: t.kwh_nano, cost_nano: t.cost_nano, cost_minor: t.cost_minor(2) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRow {
    pub state: PowerState,
    pub amount: Amount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodRow {
    pub id: String,
    pub label: String,
    pub amount: Amount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub start: DateTime<Utc>,
    /// Local wall-clock start, ISO 8601 without offset (for axis labels).
    pub local_start: NaiveDateTime,
    pub future: bool,
    pub by_state: Vec<StateRow>,
    pub total: Amount,
}

/// How much of the period's running time each estimator layer covers (per-mille).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfidenceMix {
    pub measured_sensors_permille: u32,
    pub calibrated_permille: u32,
    pub default_model_permille: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarningDto {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentInfo {
    pub state: PowerState,
    pub watts: String,
    pub low: String,
    pub high: String,
    pub layer: String,
    pub method: String,
    pub on_ac: Option<bool>,
}

impl CurrentInfo {
    pub fn new(state: PowerState, e: &Estimate, on_ac: Option<bool>) -> Self {
        CurrentInfo {
            state,
            watts: e.watts.to_string(),
            low: e.low.to_string(),
            high: e.high.to_string(),
            layer: format!("{:?}", e.layer).to_lowercase(),
            method: e.method.clone(),
            on_ac,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dashboard {
    pub period: PeriodKind,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub currency: String,
    pub minor_digits: u32,
    pub tariff_name: String,
    pub total: Amount,
    pub by_state: Vec<StateRow>,
    pub by_period: Vec<PeriodRow>,
    pub bucket: String,
    pub timeline: Vec<Point>,
    pub confidence: ConfidenceMix,
    pub warnings: Vec<WarningDto>,
    pub current: Option<CurrentInfo>,
    /// Present only when the user opted to include fixed monthly fees.
    pub fixed_fees: Option<FixedFees>,
    /// Energy cost (+ fixed fees when included), rounded once to minor units.
    pub grand_total_minor: i64,
}

fn minor_digits(currency: &str) -> u32 {
    match currency {
        "JPY" | "KRW" | "HUF" => 0,
        _ => 2,
    }
}

/// Human-readable tariff warnings (expired, unverified, stale...).
pub fn warnings_for(tariff: &Tariff, today: NaiveDate) -> Vec<WarningDto> {
    tariff.warnings(today).iter().map(warning_dto).collect()
}

fn warning_dto(w: &Warning) -> WarningDto {
    let (code, message) = match w {
        Warning::NotYetValid { valid_from } => ("not_yet_valid", format!("This tariff is valid from {valid_from}.")),
        Warning::Expired { valid_to } => {
            ("expired", format!("This tariff expired on {valid_to}; prices may have changed. Check your provider and update it."))
        }
        Warning::NeverVerified => ("never_verified", "These prices were never verified against an official source.".to_string()),
        Warning::VerifiedLongAgo { days } => {
            ("verified_long_ago", format!("Prices were last verified {days} days ago; they may be out of date."))
        }
        Warning::MarkedUnverified => ("unverified", "This preset is marked unverified - check the prices before trusting the costs.".to_string()),
    };
    WarningDto { code: code.into(), message }
}

fn confidence_mix(layers: &std::collections::BTreeMap<String, i64>) -> ConfidenceMix {
    let total: i64 = layers.values().sum();
    if total <= 0 {
        return ConfidenceMix::default();
    }
    let p = |k: &str| (layers.get(k).copied().unwrap_or(0) * 1000 / total) as u32;
    let mut mix = ConfidenceMix { measured_sensors_permille: p("sensors"), calibrated_permille: p("calibrated"), default_model_permille: p("default") };
    // make the three add up to exactly 1000 (rounding remainder goes to the largest)
    let sum = mix.measured_sensors_permille + mix.calibrated_permille + mix.default_model_permille;
    let slot = if mix.measured_sensors_permille >= mix.calibrated_permille && mix.measured_sensors_permille >= mix.default_model_permille {
        &mut mix.measured_sensors_permille
    } else if mix.calibrated_permille >= mix.default_model_permille {
        &mut mix.calibrated_permille
    } else {
        &mut mix.default_model_permille
    };
    *slot += 1000 - sum;
    mix
}

/// Every bucket start (UTC) between `from` and `end`, so the chart has no holes.
fn expected_buckets(kind: PeriodKind, from: DateTime<Utc>, end: DateTime<Utc>, tz: Tz) -> Vec<DateTime<Utc>> {
    let mut out = Vec::new();
    match bucket_for(kind) {
        Bucket::Hour => {
            let mut t = from;
            while t < end {
                out.push(t);
                t += Duration::hours(1);
            }
        }
        Bucket::Day => {
            let mut d = from.with_timezone(&tz).date_naive();
            loop {
                let s = local_midnight(d, tz);
                if s >= end {
                    break;
                }
                out.push(s);
                d = d.succ_opt().unwrap();
            }
        }
        Bucket::Month => {
            let d0 = from.with_timezone(&tz).date_naive();
            for m in 1..=12 {
                let s = local_midnight(NaiveDate::from_ymd_opt(d0.year(), m, 1).unwrap(), tz);
                if s < end {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// Display options.
#[derive(Debug, Clone, Copy, Default)]
pub struct DashOptions {
    /// Add the tariff's fixed monthly fees (prorated per day) as a separate line and to the grand total.
    pub include_fixed_fees: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedFees {
    pub cost_nano: i64,
    pub cost_minor: i64,
}

/// Fixed monthly fees (VAT included) for the local days in `[from, to)`: each day carries
/// 1/days-in-month of the monthly amount. Rounded once, at the end.
pub fn fixed_fees_nano(tariff: &Tariff, from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    use rust_decimal::prelude::ToPrimitive;
    let monthly = tariff.monthly_fixed_gross();
    if monthly.is_zero() || to <= from {
        return 0;
    }
    let tz = tariff.tz();
    let last = (to - Duration::seconds(1)).with_timezone(&tz).date_naive();
    let mut day = from.with_timezone(&tz).date_naive();
    let mut total = rust_decimal::Decimal::ZERO;
    while day <= last {
        let next_month = if day.month() == 12 { NaiveDate::from_ymd_opt(day.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(day.year(), day.month() + 1, 1) }.unwrap();
        let dim = (next_month - NaiveDate::from_ymd_opt(day.year(), day.month(), 1).unwrap()).num_days();
        total += monthly / rust_decimal::Decimal::from(dim);
        day = day.succ_opt().unwrap();
    }
    (total * rust_decimal::Decimal::from(pc_core::NANO)).round().to_i64().unwrap_or(0)
}

pub fn dashboard(
    store: &Storage,
    tariff: &Tariff,
    kind: PeriodKind,
    now: DateTime<Utc>,
    current: Option<CurrentInfo>,
) -> Result<Dashboard, DashboardError> {
    dashboard_with(store, tariff, kind, now, current, &DashOptions::default())
}

pub fn dashboard_with(
    store: &Storage,
    tariff: &Tariff,
    kind: PeriodKind,
    now: DateTime<Utc>,
    current: Option<CurrentInfo>,
    opts: &DashOptions,
) -> Result<Dashboard, DashboardError> {
    let tz = tariff.tz();
    let range = range_for(kind, now, tz);
    let summary = store.summary(range.from, range.to)?;

    let by_state = PowerState::ALL
        .iter()
        .filter_map(|s| summary.by_state.get(s).map(|t| StateRow { state: *s, amount: (*t).into() }))
        .collect();

    let by_period = tariff
        .periods
        .iter()
        .map(|p| PeriodRow { id: p.id.clone(), label: p.label.clone(), amount: summary.by_period.get(&p.id).copied().unwrap_or_default().into() })
        .collect();

    // Timeline: real buckets from storage merged into the full list of expected buckets.
    let bucket = bucket_for(kind);
    let real = store.timeline(range.from, range.to, tz, bucket)?;
    let mut by_start: std::collections::BTreeMap<_, _> = real.into_iter().map(|b| (b.start, b)).collect();
    let timeline = expected_buckets(kind, range.from, range.period_end, tz)
        .into_iter()
        .map(|start| {
            let b = by_start.remove(&start);
            let by_state: Vec<StateRow> = b
                .as_ref()
                .map(|b| PowerState::ALL.iter().filter_map(|s| b.by_state.get(s).map(|t| StateRow { state: *s, amount: (*t).into() })).collect())
                .unwrap_or_default();
            let total = b.as_ref().map(|b| b.total().into()).unwrap_or_default();
            Point { start, local_start: start.with_timezone(&tz).naive_local(), future: start >= now, by_state, total }
        })
        .collect();

    let mix = confidence_mix(&store.layer_seconds(range.from, range.to)?);
    let warnings = tariff.warnings(now.with_timezone(&tz).date_naive()).iter().map(warning_dto).collect();

    let digits = minor_digits(&tariff.currency);
    let mut d = Dashboard {
        period: kind,
        from: range.from,
        to: range.to,
        currency: tariff.currency.clone(),
        minor_digits: minor_digits(&tariff.currency),
        tariff_name: tariff.name.clone(),
        total: summary.total.into(),
        by_state,
        by_period,
        bucket: match bucket {
            Bucket::Hour => "hour",
            Bucket::Day => "day",
            Bucket::Month => "month",
        }
        .into(),
        timeline,
        confidence: mix,
        warnings,
        current,
        fixed_fees: None,
        grand_total_minor: 0,
    };
    d.apply_digits(digits);
    let fees_nano = if opts.include_fixed_fees { fixed_fees_nano(tariff, range.from, range.to) } else { 0 };
    if opts.include_fixed_fees && fees_nano > 0 {
        d.fixed_fees = Some(FixedFees { cost_nano: fees_nano, cost_minor: cost_engine::nano_to_minor(fees_nano, digits) });
    }
    d.grand_total_minor = cost_engine::nano_to_minor(d.total.cost_nano + fees_nano, digits);
    Ok(d)
}

impl Dashboard {
    /// Recompute `cost_minor` everywhere for the currency' s number of decimals.
    fn apply_digits(&mut self, digits: u32) {
        if digits == 2 {
            return;
        }
        let fix = |a: &mut Amount| *a = a.with_digits(digits);
        fix(&mut self.total);
        self.by_state.iter_mut().for_each(|r| fix(&mut r.amount));
        self.by_period.iter_mut().for_each(|r| fix(&mut r.amount));
        for p in &mut self.timeline {
            fix(&mut p.total);
            p.by_state.iter_mut().for_each(|r| fix(&mut r.amount));
        }
    }
}

