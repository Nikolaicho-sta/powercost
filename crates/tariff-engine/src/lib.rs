//! Tariff engine: loads versioned tariff JSON, validates it (JSON Schema +
//! semantic rules) and answers "which period / price applies at this UTC instant?".
//!
//! Design rules (see docs/DECISIONS.md):
//! * Schedules are expressed in LOCAL wall-clock time of the tariff's IANA timezone.
//!   Each UTC instant is converted to local time, so DST needs no special casing.
//! * Time ranges never wrap midnight; split them (22:00-24:00 + 00:00-06:00).
//! * For every season x day type, every minute of the day must be covered exactly once.
//! * Money is `rust_decimal::Decimal`, never floats.

mod catalog;
mod model;
mod region;

pub use catalog::*;
pub use model::*;
pub use region::*;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use rust_decimal::Decimal;
use std::collections::HashSet;

const SCHEMA_JSON: &str = include_str!("../../../schema/tariff.schema.json");

#[derive(Debug, thiserror::Error)]
pub enum TariffError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("schema validation failed: {}", .0.join("; "))]
    Schema(Vec<String>),
    #[error("invalid tariff: {0}")]
    Invalid(String),
}

fn invalid<T>(msg: impl Into<String>) -> Result<T, TariffError> {
    Err(TariffError::Invalid(msg.into()))
}

/// What applies at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<'a> {
    pub period_id: &'a str,
    pub period_index: usize,
    pub season_id: &'a str,
    pub local: NaiveDateTime,
}

/// Price per kWh of one period. `gross` is what the household actually pays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodPrice {
    pub excl_vat: Decimal,
    pub vat: Decimal,
    pub gross: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    NotYetValid { valid_from: NaiveDate },
    Expired { valid_to: NaiveDate },
    NeverVerified,
    VerifiedLongAgo { days: i64 },
    MarkedUnverified,
    /// Prices usually change on `typical_update`; that date has passed since this version started
    /// and the prices were not re-checked after it.
    UpdateLikelyDue { since: NaiveDate },
}

impl Tariff {
    pub fn from_json(json: &str) -> Result<Tariff, TariffError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        Self::from_value(value)
    }

    pub fn from_value(value: serde_json::Value) -> Result<Tariff, TariffError> {
        validate_schema(&value)?;
        let mut tariff: Tariff = serde_json::from_value(value)?;
        tariff.compiled = Some(compile(&tariff)?);
        Ok(tariff)
    }

    fn c(&self) -> &Compiled {
        self.compiled.as_ref().expect("tariff is always compiled by from_json/from_value")
    }

    pub fn tz(&self) -> Tz {
        self.c().tz
    }

    /// Period and season in force at a UTC instant.
    pub fn resolve(&self, utc: DateTime<Utc>) -> Resolved<'_> {
        let c = self.c();
        let local = utc.with_timezone(&c.tz).naive_local();
        let date = local.date();
        let md = (date.month() as u8, date.day() as u8);
        let minute = (local.hour() * 60 + local.minute()) as u16;
        let day_type = self.day_type(date);

        let season = c
            .seasons
            .iter()
            .find(|s| s.contains(md))
            .expect("validated: every day belongs to a season");
        let rule = season
            .rules
            .iter()
            .find(|r| r.days.contains(&day_type) && r.ranges.iter().any(|&(s, e)| s <= minute && minute < e))
            .expect("validated: every minute is covered");
        Resolved {
            period_id: &self.periods[rule.period].id,
            period_index: rule.period,
            season_id: &season.id,
            local,
        }
    }

    /// Holidays win over weekend/weekday.
    pub fn day_type(&self, date: NaiveDate) -> DayType {
        if self.c().holidays.contains(&date) {
            return DayType::Holiday;
        }
        match date.weekday() {
            Weekday::Sat => DayType::Saturday,
            Weekday::Sun => DayType::Sunday,
            _ => DayType::Weekday,
        }
    }

    pub fn price_for_index(&self, period_index: usize) -> PeriodPrice {
        self.c().prices[period_index]
    }

    pub fn price(&self, period_id: &str) -> Option<PeriodPrice> {
        self.periods.iter().position(|p| p.id == period_id).map(|i| self.c().prices[i])
    }

    /// Fixed monthly fees, VAT included. Excluded from per-kWh cost by default.
    pub fn monthly_fixed_gross(&self) -> Decimal {
        let f = Decimal::ONE + self.vat_percent / Decimal::from(100);
        self.fixed_fees
            .iter()
            .map(|x| if x.vat_inclusive { x.per_month } else { x.per_month * f })
            .sum()
    }

    pub fn warnings(&self, today: NaiveDate) -> Vec<Warning> {
        let mut w = Vec::new();
        if today < self.valid_from {
            w.push(Warning::NotYetValid { valid_from: self.valid_from });
        }
        if let Some(to) = self.valid_to {
            if today > to {
                w.push(Warning::Expired { valid_to: to });
            }
        }
        match self.last_verified {
            None => w.push(Warning::NeverVerified),
            Some(d) => {
                let days = (today - d).num_days();
                if days > 365 {
                    w.push(Warning::VerifiedLongAgo { days });
                }
            }
        }
        if matches!(self.verification, Some(Verification::Unverified)) {
            w.push(Warning::MarkedUnverified);
        }
        if let Some(since) = self.last_typical_update(today) {
            if self.valid_from < since && self.last_verified.is_none_or(|v| v < since) {
                w.push(Warning::UpdateLikelyDue { since });
            }
        }
        w
    }

    /// The most recent occurrence of `typical_update` on or before `today`.
    pub fn last_typical_update(&self, today: NaiveDate) -> Option<NaiveDate> {
        let (m, d) = parse_md(self.typical_update.as_deref()?);
        let this_year = NaiveDate::from_ymd_opt(today.year(), m as u32, d as u32)?;
        if this_year <= today {
            Some(this_year)
        } else {
            NaiveDate::from_ymd_opt(today.year() - 1, m as u32, d as u32)
        }
    }
}

fn validate_schema(instance: &serde_json::Value) -> Result<(), TariffError> {
    let schema: serde_json::Value = serde_json::from_str(SCHEMA_JSON).expect("embedded schema is valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("embedded schema compiles");
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("{} (at {})", e, e.instance_path()))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(TariffError::Schema(errors))
    }
}

fn parse_hhmm(s: &str) -> u16 {
    // Schema guarantees the format; "24:00" -> 1440.
    let (h, m) = s.split_once(':').expect("schema-validated HH:MM");
    h.parse::<u16>().unwrap() * 60 + m.parse::<u16>().unwrap()
}

fn parse_md(s: &str) -> (u8, u8) {
    let (m, d) = s.split_once('-').expect("schema-validated MM-DD");
    (m.parse().unwrap(), d.parse().unwrap())
}

fn compile(t: &Tariff) -> Result<Compiled, TariffError> {
    let tz: Tz = t.timezone.parse().map_err(|_| TariffError::Invalid(format!("unknown timezone '{}'", t.timezone)))?;

    if t.vat_percent < Decimal::ZERO || t.vat_percent > Decimal::from(100) {
        return invalid("vat_percent must be between 0 and 100");
    }
    if let Some(to) = t.valid_to {
        if to < t.valid_from {
            return invalid("valid_to is before valid_from");
        }
    }
    if let Some(tu) = &t.typical_update {
        let (m, d) = parse_md(tu);
        if NaiveDate::from_ymd_opt(2024, m as u32, d as u32).is_none() {
            return invalid(format!("typical_update '{tu}' is not a real month-day"));
        }
    }

    // periods
    let mut ids = HashSet::new();
    for p in &t.periods {
        if !ids.insert(p.id.as_str()) {
            return invalid(format!("duplicate period id '{}'", p.id));
        }
    }
    let period_index = |id: &str| t.periods.iter().position(|p| p.id == id);

    // components -> prices
    let vat = t.vat_percent / Decimal::from(100);
    let factor = Decimal::ONE + vat;
    let mut prices = Vec::new();
    for p in &t.periods {
        let (mut excl, mut gross) = (Decimal::ZERO, Decimal::ZERO);
        for c in &t.components {
            let v = match (&c.per_kwh, &c.per_period) {
                (Some(v), None) => *v,
                (None, Some(map)) => match map.get(&p.id) {
                    Some(v) => *v,
                    None => return invalid(format!("component '{}' has no price for period '{}'", c.id, p.id)),
                },
                _ => return invalid(format!("component '{}' must set exactly one of per_kwh / per_period", c.id)),
            };
            if v < Decimal::ZERO {
                return invalid(format!("component '{}' has a negative price", c.id));
            }
            if c.vat_inclusive {
                gross += v;
                excl += v / factor;
            } else {
                excl += v;
                gross += v * factor;
            }
        }
        prices.push(PeriodPrice { excl_vat: excl, vat: gross - excl, gross });
    }
    for c in &t.components {
        if let Some(map) = &c.per_period {
            for k in map.keys() {
                if period_index(k).is_none() {
                    return invalid(format!("component '{}' references unknown period '{}'", c.id, k));
                }
            }
        }
    }

    // seasons & rules
    let mut seasons = Vec::new();
    for s in &t.schedule.seasons {
        let mut rules = Vec::new();
        for r in &s.rules {
            let Some(pi) = period_index(&r.period) else {
                return invalid(format!("season '{}' rule references unknown period '{}'", s.id, r.period));
            };
            let mut days = Vec::new();
            for d in &r.days {
                days.extend(d.expand());
            }
            let mut ranges = Vec::new();
            for rg in &r.ranges {
                let (a, b) = (parse_hhmm(&rg.start), parse_hhmm(&rg.end));
                if a >= b {
                    return invalid(format!(
                        "season '{}': range {}-{} must have start < end (split ranges at midnight)",
                        s.id, rg.start, rg.end
                    ));
                }
                ranges.push((a, b));
            }
            rules.push(CRule { days, ranges, period: pi });
        }
        seasons.push(CSeason { id: s.id.clone(), from: parse_md(&s.from), to: parse_md(&s.to), rules });
    }

    // every minute covered exactly once, per season x day type
    for s in &seasons {
        for dt in DayType::ALL {
            let mut cover = vec![0u8; 1440];
            for r in s.rules.iter().filter(|r| r.days.contains(&dt)) {
                for &(a, b) in &r.ranges {
                    for m in a..b {
                        cover[m as usize] = cover[m as usize].saturating_add(1);
                    }
                }
            }
            if let Some(m) = cover.iter().position(|&n| n != 1) {
                let what = if cover[m] == 0 { "not covered" } else { "covered more than once" };
                return invalid(format!(
                    "season '{}', day type {:?}: minute {:02}:{:02} is {}",
                    s.id,
                    dt,
                    m / 60,
                    m % 60,
                    what
                ));
            }
        }
    }

    // every calendar day (leap year) belongs to exactly one season
    let mut d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
    while d.year() == 2024 {
        let md = (d.month() as u8, d.day() as u8);
        let n = seasons.iter().filter(|s| s.contains(md)).count();
        if n != 1 {
            return invalid(format!("{:02}-{:02} belongs to {} seasons (must be exactly 1)", md.0, md.1, n));
        }
        d = d.succ_opt().unwrap();
    }

    Ok(Compiled { tz, seasons, prices, holidays: t.holidays.iter().copied().collect() })
}
