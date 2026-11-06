//! Application logic that is not tied to the UI toolkit (and therefore tested): which tariff is
//! active, saving a tariff and repricing history, calibration with re-estimation, budget alerts,
//! compaction schedule, and the tariff editor's live validation.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use cost_engine::{nano_to_minor, TariffSet};
use dashboard::{fixed_fees_nano, range_for, PeriodKind, WarningDto};
use estimator::{Calibration, CalibrationPoint, Estimator};
use serde::{Deserialize, Serialize};
use settings::Settings;
use storage::{CompactionReport, RecomputeReport, Storage};
use tariff_engine::{Catalog, PresetInfo, Tariff, TariffError};

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("{0}")]
    Message(String),
    #[error("invalid settings: {}", .0.join("; "))]
    Settings(Vec<String>),
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
    #[error(transparent)]
    Tariff(#[from] TariffError),
    #[error(transparent)]
    Cost(#[from] cost_engine::CostError),
}

fn msg<T>(s: impl Into<String>) -> Result<T, BackendError> {
    Err(BackendError::Message(s.into()))
}

// ---------------------------------------------------------------- active tariff

/// The tariff versions to price with, plus the version in force today. Falls back to the first
/// available tariff when the chosen one disappeared (a deleted user file), and says so.
pub struct ActiveTariff {
    pub current: Tariff,
    pub set: TariffSet,
    pub fallback_note: Option<String>,
}

pub fn active_tariff(catalog: &Catalog, settings: &Settings, today: NaiveDate) -> Result<ActiveTariff, BackendError> {
    let pick = |id: &str| -> Option<(Tariff, Vec<Tariff>)> {
        let versions = catalog.versions(id);
        let current = catalog.current(id, today)?;
        Some((current, versions))
    };
    let (id, note, (current, versions)) = match pick(&settings.tariff_id) {
        Some(x) => (settings.tariff_id.clone(), None, x),
        None => {
            let Some(first) = catalog.latest().into_iter().next() else { return msg("no tariffs are available") };
            let note = format!("Tariff '{}' was not found; using '{}' instead. Choose a tariff in Settings.", settings.tariff_id, first.id);
            let x = pick(&first.id).expect("listed tariff exists");
            (first.id, Some(note), x)
        }
    };
    let _ = id;
    Ok(ActiveTariff { current, set: TariffSet::new(versions)?, fallback_note: note })
}

// ---------------------------------------------------------------- tariff editor

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricePreview {
    pub period_id: String,
    pub label: String,
    pub excl_vat: String,
    pub gross: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<WarningDto>,
    /// Resulting price per kWh by period (what the editor shows next to the inputs).
    pub prices: Vec<PricePreview>,
}

/// Live validation for the tariff editor: all schema/semantic errors at once, a price preview,
/// and the staleness warnings.
pub fn validate_tariff(json: serde_json::Value, today: NaiveDate) -> ValidationReport {
    match Tariff::from_value(json) {
        Ok(t) => {
            let prices = t
                .periods
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let price = t.price_for_index(i);
                    PricePreview { period_id: p.id.clone(), label: p.label.clone(), excl_vat: price.excl_vat.round_dp(6).normalize().to_string(), gross: price.gross.round_dp(6).normalize().to_string() }
                })
                .collect();
            ValidationReport { ok: true, errors: vec![], warnings: dashboard::warnings_for(&t, today), prices }
        }
        Err(e) => {
            let errors = match e {
                TariffError::Schema(v) => v,
                other => vec![other.to_string()],
            };
            ValidationReport { ok: false, errors, warnings: vec![], prices: vec![] }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveTariffResult {
    pub info: PresetInfo,
    /// Present when the saved tariff is the active one and history was repriced.
    pub recomputed_spans: Option<usize>,
    pub clipped_by_compaction: bool,
}

fn local_midnight_utc(date: NaiveDate, tz: Tz) -> DateTime<Utc> {
    let mut naive = date.and_time(NaiveTime::MIN);
    loop {
        if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
            return dt.with_timezone(&Utc);
        }
        naive += Duration::minutes(30);
    }
}

/// Save a user tariff (a new tariff or a new version) and, when it is the active one, reprice the
/// detailed history from `recompute_from` (default: the version's `valid_from`) until now.
pub fn save_tariff(
    store: &mut Storage,
    catalog: &mut Catalog,
    user_dir: &std::path::Path,
    json: serde_json::Value,
    active_id: &str,
    recompute_from: Option<NaiveDate>,
    now: DateTime<Utc>,
) -> Result<SaveTariffResult, BackendError> {
    let saved = catalog.save_user(user_dir, json)?;
    let info = catalog.list().into_iter().find(|p| p.id == saved.id && p.version == saved.version).expect("just saved");
    let mut out = SaveTariffResult { info, recomputed_spans: None, clipped_by_compaction: false };
    if saved.id == active_id {
        let set = TariffSet::new(catalog.versions(&saved.id))?;
        let from = local_midnight_utc(recompute_from.unwrap_or(saved.valid_from), saved.tz());
        if from < now {
            let rep: RecomputeReport = store.recompute_costs(from, now, &set)?;
            out.recomputed_spans = Some(rep.spans);
            out.clipped_by_compaction = rep.clipped_by_compaction;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- calibration

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReestimateSummary {
    pub spans: usize,
    pub clipped_by_compaction: bool,
}

/// Store a wall-meter calibration (`points` include idle at 0 and full at 1000 per-mille) and
/// re-estimate the last `reestimate_days` of detailed history with it. `None` clears it.
pub fn set_calibration(
    store: &mut Storage,
    settings: &mut Settings,
    points: Option<Vec<CalibrationPoint>>,
    reestimate_days: u32,
    tariffs: &TariffSet,
    now: DateTime<Utc>,
) -> Result<ReestimateSummary, BackendError> {
    let mut next = settings.clone();
    next.estimator.calibration = match points {
        Some(p) => Some(Calibration::from_points(p).map_err(|e| BackendError::Message(e.to_string()))?),
        None => None,
    };
    next.save(store).map_err(BackendError::Settings)?;
    *settings = next;
    let est = Estimator::new(settings.estimator.clone()).map_err(|e| BackendError::Message(e.to_string()))?;
    let rep = store.reestimate(now - Duration::days(reestimate_days.min(3650) as i64), now, &est, tariffs)?;
    Ok(ReestimateSummary { spans: rep.spans, clipped_by_compaction: rep.clipped_by_compaction })
}

// ---------------------------------------------------------------- budget

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetAlert {
    pub percent: u32,
    pub title: String,
    pub body: String,
}

fn money_text(minor: i64, digits: u32, currency: &str) -> String {
    let unit = 10i64.pow(digits);
    if digits == 0 {
        format!("{minor} {currency}")
    } else {
        format!("{}.{:0width$} {currency}", minor / unit, (minor % unit).abs(), width = digits as usize)
    }
}

/// Check this month's estimated cost against the budget. Returns one alert for the highest
/// newly reached threshold and records every reached threshold as notified (once per month).
pub fn check_budget(store: &Storage, settings: &Settings, tariff: &Tariff, now: DateTime<Utc>) -> Result<Option<BudgetAlert>, BackendError> {
    let Some(budget) = &settings.budget else { return Ok(None) };
    let tz = tariff.tz();
    let range = range_for(PeriodKind::Month, now, tz);
    let digits = if matches!(tariff.currency.as_str(), "JPY" | "KRW" | "HUF") { 0 } else { 2 };
    let mut nano = store.summary(range.from, now)?.total.cost_nano;
    if settings.include_fixed_fees {
        nano += fixed_fees_nano(tariff, range.from, now);
    }
    let spent_minor = nano_to_minor(nano, digits);

    let local = now.with_timezone(&tz).date_naive();
    use chrono::Datelike;
    let key = settings::month_key(local.year(), local.month());
    let due = settings::due_alerts(budget, spent_minor, &settings::load_sent(store, &key)?);
    let Some(&percent) = due.last() else { return Ok(None) };
    settings::mark_sent(store, &key, &due)?;
    let (title, body) = settings::alert_text(percent, &money_text(spent_minor, digits, &tariff.currency), &money_text(budget.monthly_minor, digits, &tariff.currency));
    Ok(Some(BudgetAlert { percent, title, body }))
}

// ---------------------------------------------------------------- compaction

/// Compact detail older than the retention setting. Idempotent; call at start and every few hours.
pub fn compact_if_due(store: &mut Storage, settings: &Settings, tz: Tz, now: DateTime<Utc>) -> Result<CompactionReport, BackendError> {
    Ok(store.compact(now - Duration::days(settings.detail_retention_days as i64), tz)?)
}
