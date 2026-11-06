use crate::PeriodPrice;
use chrono::NaiveDate;
use chrono_tz::Tz;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verification {
    Verified,
    Partial,
    Unverified,
    Example,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentKind {
    Energy,
    Network,
    Excise,
    Levy,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DayKind {
    All,
    Weekday,
    Weekend,
    Saturday,
    Sunday,
    Holiday,
}

/// Concrete day classes used during lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DayType {
    Weekday,
    Saturday,
    Sunday,
    Holiday,
}

impl DayType {
    pub const ALL: [DayType; 4] = [DayType::Weekday, DayType::Saturday, DayType::Sunday, DayType::Holiday];
}

impl DayKind {
    pub(crate) fn expand(self) -> Vec<DayType> {
        match self {
            DayKind::All => DayType::ALL.to_vec(),
            DayKind::Weekday => vec![DayType::Weekday],
            DayKind::Weekend => vec![DayType::Saturday, DayType::Sunday],
            DayKind::Saturday => vec![DayType::Saturday],
            DayKind::Sunday => vec![DayType::Sunday],
            DayKind::Holiday => vec![DayType::Holiday],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Component {
    pub id: String,
    pub kind: ComponentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub vat_inclusive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_kwh: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_period: Option<BTreeMap<String, Decimal>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixedFee {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub per_month: Decimal,
    pub vat_inclusive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Period {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub days: Vec<DayKind>,
    pub ranges: Vec<TimeRange>,
    pub period: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Season {
    pub id: String,
    /// "MM-DD", inclusive. A season may wrap the year end (from > to).
    pub from: String,
    pub to: String,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub seasons: Vec<Season>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tariff {
    pub schema_version: u32,
    pub id: String,
    #[serde(default = "one")]
    pub version: u32,
    pub name: String,
    pub country: String,
    #[serde(default)]
    pub distributor: Option<String>,
    pub currency: String,
    pub timezone: String,
    pub vat_percent: Decimal,
    pub valid_from: NaiveDate,
    #[serde(default)]
    pub valid_to: Option<NaiveDate>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub last_verified: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub components: Vec<Component>,
    #[serde(default)]
    pub fixed_fees: Vec<FixedFee>,
    pub periods: Vec<Period>,
    #[serde(default)]
    pub holidays: Vec<NaiveDate>,
    pub schedule: Schedule,
    #[serde(skip)]
    pub(crate) compiled: Option<Compiled>,
}

#[derive(Debug, Clone)]
pub(crate) struct CRule {
    pub days: Vec<DayType>,
    pub ranges: Vec<(u16, u16)>,
    pub period: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct CSeason {
    pub id: String,
    pub from: (u8, u8),
    pub to: (u8, u8),
    pub rules: Vec<CRule>,
}

impl CSeason {
    pub fn contains(&self, md: (u8, u8)) -> bool {
        if self.from <= self.to {
            self.from <= md && md <= self.to
        } else {
            md >= self.from || md <= self.to
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Compiled {
    pub tz: Tz,
    pub seasons: Vec<CSeason>,
    pub prices: Vec<PeriodPrice>,
    pub holidays: HashSet<NaiveDate>,
}
