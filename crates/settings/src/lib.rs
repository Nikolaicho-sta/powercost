//! User settings (one JSON document in the database) and the monthly-budget alert logic.
//! Everything has a safe default so an old or partial document still loads.

pub mod autostart;

use gap_classifier::Assumptions;
use estimator::EstimatorConfig;
use pc_core::PowerState;
use serde::{Deserialize, Serialize};
use state_detection::Thresholds;
use std::collections::BTreeSet;
use storage::Storage;

pub const SETTINGS_KEY: &str = "app_settings";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RegionSetting {
    /// ISO 3166-1 alpha-2, confirmed or chosen by the user.
    pub country: Option<String>,
    pub confirmed: bool,
    /// The user agreed to the optional IP lookup (a network call). Off by default.
    pub ip_lookup_opt_in: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// Monthly budget in minor units (cents).
    pub monthly_minor: i64,
    /// Notify when the month's cost reaches these percentages of the budget.
    pub alert_percents: Vec<u32>,
}

impl Default for Budget {
    fn default() -> Self {
        Self { monthly_minor: 1000, alert_percents: vec![80, 100] }
    }
}

/// What to assume for time the tracker could not explain (see gap-classifier).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GapAssumptions {
    pub crash: PowerState,
    pub unknown: PowerState,
}

impl Default for GapAssumptions {
    fn default() -> Self {
        Self { crash: PowerState::Off, unknown: PowerState::Off }
    }
}

impl GapAssumptions {
    pub fn to_classifier(self) -> Assumptions {
        Assumptions { crash: self.crash, unknown: self.unknown, ..Assumptions::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub first_run_done: bool,
    pub region: RegionSetting,
    pub tariff_id: String,
    pub estimator: EstimatorConfig,
    pub thresholds: Thresholds,
    pub gap_assumptions: GapAssumptions,
    pub budget: Option<Budget>,
    /// Add the tariff's fixed monthly fees to the totals (off by default).
    pub include_fixed_fees: bool,
    pub autostart: bool,
    pub theme: Theme,
    /// How long minute-level detail is kept before it is compacted into hourly/daily totals.
    pub detail_retention_days: u32,
    /// Look for a newer release on GitHub about once a day and show a notification (a network call;
    /// ON by default at the user's decision, disclosed in the wizard/Settings/PRIVACY.md, one switch
    /// turns it off). Pressing "Check for updates" always works; this only controls the automatic check.
    pub online_update_check: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            first_run_done: false,
            region: RegionSetting::default(),
            tariff_id: "bg-electrohold".into(),
            estimator: EstimatorConfig::default(),
            thresholds: Thresholds::default(),
            gap_assumptions: GapAssumptions::default(),
            budget: None,
            include_fixed_fees: false,
            autostart: false,
            theme: Theme::System,
            detail_retention_days: 90,
            online_update_check: true,
        }
    }
}

impl Settings {
    /// All problems at once, so the form can show them together.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut e = Vec::new();
        if let Err(x) = self.estimator.validate() {
            e.push(x.to_string());
        }
        let t = &self.thresholds;
        if !(10..=86_400).contains(&t.input_idle_secs) {
            e.push("idle-after must be between 10 seconds and 24 hours".into());
        }
        for (name, enter, exit) in [("CPU", t.cpu_enter_percent, t.cpu_exit_percent), ("GPU", t.gpu_enter_percent, t.gpu_exit_percent)] {
            if enter > 100 || exit > 100 {
                e.push(format!("{name} thresholds must be 0-100%"));
            } else if exit > enter {
                e.push(format!("{name}: the level to stay active must not be above the level to become active"));
            }
        }
        if !(1..=20).contains(&t.exit_debounce_ticks) {
            e.push("debounce must be 1-20 samples".into());
        }
        if !(7..=3650).contains(&self.detail_retention_days) {
            e.push("detail retention must be between 7 and 3650 days".into());
        }
        if self.tariff_id.trim().is_empty() {
            e.push("choose a tariff".into());
        }
        if let Some(c) = &self.region.country {
            if c.len() != 2 || !c.chars().all(|x| x.is_ascii_uppercase()) {
                e.push("country must be a two-letter code like BG".into());
            }
        }
        if let Some(b) = &self.budget {
            if b.monthly_minor <= 0 {
                e.push("monthly budget must be above zero".into());
            }
            if b.alert_percents.is_empty() || b.alert_percents.len() > 10 || b.alert_percents.iter().any(|p| !(1..=1000).contains(p)) {
                e.push("budget alerts need 1-10 percentages between 1 and 1000".into());
            }
        }
        if e.is_empty() {
            Ok(())
        } else {
            Err(e)
        }
    }

    pub fn load(store: &Storage) -> Result<Settings, storage::StorageError> {
        Ok(store.get_setting::<Settings>(SETTINGS_KEY)?.unwrap_or_default())
    }

    /// Validates first; an invalid document is never written.
    pub fn save(&self, store: &Storage) -> Result<(), Vec<String>> {
        self.validate()?;
        store.set_setting(SETTINGS_KEY, self).map_err(|e| vec![e.to_string()])
    }
}

// ---------------------------------------------------------------- budget alerts

/// "2026-11": alerts are tracked per calendar month so each threshold fires once a month.
pub fn month_key(year: i32, month: u32) -> String {
    format!("{year:04}-{month:02}")
}

/// Thresholds that the month's spend has reached and that were not notified yet, ascending.
pub fn due_alerts(budget: &Budget, spent_minor: i64, already_sent: &BTreeSet<u32>) -> Vec<u32> {
    if budget.monthly_minor <= 0 {
        return Vec::new();
    }
    let mut v: Vec<u32> = budget
        .alert_percents
        .iter()
        .copied()
        .filter(|p| !already_sent.contains(p) && (spent_minor as i128) * 100 >= (budget.monthly_minor as i128) * (*p as i128))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn sent_key(month: &str) -> String {
    format!("budget_alerts_sent:{month}")
}

pub fn load_sent(store: &Storage, month: &str) -> Result<BTreeSet<u32>, storage::StorageError> {
    Ok(store.get_setting::<BTreeSet<u32>>(&sent_key(month))?.unwrap_or_default())
}

pub fn mark_sent(store: &Storage, month: &str, percents: &[u32]) -> Result<(), storage::StorageError> {
    let mut s = load_sent(store, month)?;
    s.extend(percents.iter().copied());
    store.set_setting(&sent_key(month), &s)
}

/// What to say. Only the highest newly reached threshold produces a message, so a jump from
/// 50% to 120% gives one notification, not two.
pub fn alert_text(percent: u32, spent: &str, budget: &str) -> (String, String) {
    if percent >= 100 {
        ("Monthly budget reached".into(), format!("Estimated electricity cost this month is {spent} ({percent}% of your {budget} budget)."))
    } else {
        (format!("{percent}% of your monthly budget"), format!("Estimated cost this month: {spent} of {budget}."))
    }
}