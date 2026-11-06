//! Tauri commands: thin wrappers over `backend`, `settings`, `dashboard` and `storage`.
//! Arguments arrive camelCase from JavaScript (`recomputeFrom`) and snake_case here.

use crate::state::{AppState, Live};
use backend::{ReestimateSummary, SaveTariffResult, ValidationReport};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use cost_engine::TariffSet;
use dashboard::{Dashboard, DashOptions, ExportFormat, Granularity, PeriodKind, WhatIfParams, WhatIfResult};
use estimator::CalibrationPoint;
use serde::Serialize;
use settings::autostart::RunKey;
use settings::Settings;
use storage::Storage;
use tariff_engine::{guess_region, Candidate, PresetInfo};
use tauri::State;

type CmdResult<T> = Result<T, String>;

fn open(state: &AppState) -> CmdResult<Storage> {
    Storage::open(&state.db_path).map_err(|e| e.to_string())
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Re-resolve the active tariff after settings or the catalogue changed and wake the tracker.
fn refresh_live(state: &AppState, settings: Settings) -> CmdResult<()> {
    let catalog = lock(&state.catalog);
    let mut live = lock(&state.live);
    let version = live.version + 1;
    *live = Live::resolve(&catalog, settings, version)?;
    Ok(())
}

#[tauri::command(async)]
pub fn get_dashboard(state: State<AppState>, period: PeriodKind) -> CmdResult<Dashboard> {
    let store = open(&state)?;
    let current = lock(&state.shared).current.clone();
    let (tariff, opts) = {
        let l = lock(&state.live);
        (l.tariff.clone(), DashOptions { include_fixed_fees: l.settings.include_fixed_fees })
    };
    dashboard::dashboard_with(&store, &tariff, period, Utc::now(), current, &opts).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn get_what_if(state: State<AppState>, params: WhatIfParams, days: u32) -> CmdResult<WhatIfResult> {
    let store = open(&state)?;
    let tariff = lock(&state.live).tariff.clone();
    let to = Utc::now();
    dashboard::what_if(&store, &tariff, &params, to - Duration::days(days.clamp(1, 90) as i64), to).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct AppInfo {
    simulated: bool,
    preview: bool,
    version: &'static str,
    tracker_running: bool,
    sensor_issues: Vec<String>,
    data_dir: String,
    tariff_note: Option<String>,
    autostart_enabled: bool,
}

#[tauri::command]
pub fn get_app_info(state: State<AppState>) -> AppInfo {
    let shared = lock(&state.shared);
    AppInfo {
        simulated: state.simulated,
        preview: false,
        version: env!("CARGO_PKG_VERSION"),
        tracker_running: shared.tracker_running,
        sensor_issues: shared.issues.clone(),
        data_dir: state.data_dir.display().to_string(),
        tariff_note: lock(&state.live).tariff_note.clone(),
        autostart_enabled: RunKey::windows_default().is_enabled(),
    }
}

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Settings {
    lock(&state.live).settings.clone()
}

/// Validate, apply autostart, persist and push to the running tracker.
#[tauri::command(async)]
pub fn save_settings(state: State<AppState>, settings: Settings) -> Result<Settings, Vec<String>> {
    settings.validate()?;
    let previous = lock(&state.live).settings.autostart;
    if settings.autostart != previous && !state.simulated {
        let key = RunKey::windows_default();
        let res = if settings.autostart {
            std::env::current_exe().map_err(|e| vec![e.to_string()]).and_then(|exe| key.enable(&exe, "--tray").map_err(|e| vec![e]))
        } else {
            key.disable().map_err(|e| vec![e])
        };
        res?;
    }
    let store = open(&state).map_err(|e| vec![e])?;
    settings.save(&store)?;
    refresh_live(&state, settings.clone()).map_err(|e| vec![e])?;
    Ok(settings)
}

// ---------------------------------------------------------------- tariffs

#[derive(Serialize)]
pub struct TariffList {
    presets: Vec<PresetInfo>,
    problems: Vec<String>,
    selected_id: String,
    user_dir: String,
}

#[tauri::command]
pub fn list_tariffs(state: State<AppState>) -> TariffList {
    let catalog = lock(&state.catalog);
    TariffList {
        presets: catalog.list(),
        problems: catalog.problems.clone(),
        selected_id: lock(&state.live).settings.tariff_id.clone(),
        user_dir: state.tariff_dir().display().to_string(),
    }
}

#[tauri::command]
pub fn get_tariff(state: State<AppState>, id: String, version: u32) -> CmdResult<serde_json::Value> {
    let catalog = lock(&state.catalog);
    let t = catalog.get(&id, version).ok_or_else(|| format!("tariff {id} v{version} not found"))?;
    serde_json::to_value(t).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn validate_tariff(json: serde_json::Value) -> ValidationReport {
    backend::validate_tariff(json, Utc::now().date_naive())
}

#[tauri::command(async)]
pub fn save_tariff(state: State<AppState>, json: serde_json::Value, recompute_from: Option<String>) -> CmdResult<SaveTariffResult> {
    let from = recompute_from.as_deref().map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| e.to_string())).transpose()?;
    let mut store = open(&state)?;
    let active_id = lock(&state.live).settings.tariff_id.clone();
    let result = {
        let mut catalog = lock(&state.catalog);
        backend::save_tariff(&mut store, &mut catalog, &state.tariff_dir(), json, &active_id, from, Utc::now()).map_err(|e| e.to_string())?
    };
    let settings = lock(&state.live).settings.clone();
    refresh_live(&state, settings)?;
    Ok(result)
}

#[tauri::command]
pub fn delete_tariff(state: State<AppState>, id: String, version: u32) -> CmdResult<()> {
    if lock(&state.live).settings.tariff_id == id {
        return Err("this is the active tariff; select another one first".into());
    }
    lock(&state.catalog).delete_user(&state.tariff_dir(), &id, version)
}

#[derive(Serialize)]
pub struct RegionInfo {
    candidates: Vec<Candidate>,
    /// Presets for the best candidate country (empty if none ship for it).
    suggestions: Vec<PresetInfo>,
}

#[tauri::command]
pub fn detect_region(state: State<AppState>, locale: String, timezone: String) -> RegionInfo {
    let guess = guess_region(&locale, &timezone);
    let catalog = lock(&state.catalog);
    let suggestions = guess.best().map(|c| catalog.for_country(c)).unwrap_or_default();
    RegionInfo { candidates: guess.candidates, suggestions }
}

#[tauri::command]
pub fn presets_for_country(state: State<AppState>, country: String) -> Vec<PresetInfo> {
    lock(&state.catalog).for_country(&country)
}

// ---------------------------------------------------------------- calibration / history

#[tauri::command(async)]
pub fn save_calibration(state: State<AppState>, points: Option<Vec<CalibrationPoint>>, days: u32) -> CmdResult<ReestimateSummary> {
    let mut store = open(&state)?;
    let (mut settings, versions) = {
        let l = lock(&state.live);
        (l.settings.clone(), l.versions.clone())
    };
    let set = TariffSet::new(versions).map_err(|e| e.to_string())?;
    let summary = backend::set_calibration(&mut store, &mut settings, points, days, &set, Utc::now()).map_err(|e| e.to_string())?;
    refresh_live(&state, settings)?;
    Ok(summary)
}

/// Reprice the last `days` of detailed history with the active tariff (e.g. after switching tariff).
#[tauri::command(async)]
pub fn reprice_history(state: State<AppState>, days: u32) -> CmdResult<usize> {
    let mut store = open(&state)?;
    let versions = lock(&state.live).versions.clone();
    let set = TariffSet::new(versions).map_err(|e| e.to_string())?;
    let now = Utc::now();
    let rep = store.recompute_costs(now - Duration::days(days.clamp(1, 3650) as i64), now, &set).map_err(|e| e.to_string())?;
    Ok(rep.spans)
}

// ---------------------------------------------------------------- export / notifications

fn local_midnight(date: NaiveDate, tz: chrono_tz::Tz) -> DateTime<Utc> {
    let mut naive = date.and_hms_opt(0, 0, 0).expect("midnight");
    loop {
        if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
            return dt.with_timezone(&Utc);
        }
        naive += Duration::minutes(30);
    }
}

/// Write an export of `[from_date, to_date]` (local dates, inclusive) to `path` (chosen with the
/// system save dialog). Only `.csv` / `.json` files are written.
#[tauri::command(async)]
pub fn export_data(state: State<AppState>, path: String, format: ExportFormat, granularity: Granularity, from_date: String, to_date: String) -> CmdResult<String> {
    let ext = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext != "csv" && ext != "json" {
        return Err("choose a .csv or .json file name".into());
    }
    let parse = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| format!("bad date '{s}': {e}"));
    let (d0, d1) = (parse(&from_date)?, parse(&to_date)?);
    if d1 < d0 {
        return Err("the end date is before the start date".into());
    }
    let tariff = lock(&state.live).tariff.clone();
    let tz = tariff.tz();
    let (from, to) = (local_midnight(d0, tz), local_midnight(d1.succ_opt().expect("date"), tz));
    let store = open(&state)?;
    let text = dashboard::export(&store, &tariff, from, to, granularity, format, Utc::now()).map_err(|e| e.to_string())?;
    std::fs::write(&path, text.as_bytes()).map_err(|e| format!("cannot write {path}: {e}"))?;
    Ok(path)
}

#[tauri::command]
pub fn send_test_notification(app: tauri::AppHandle) -> CmdResult<()> {
    use tauri_plugin_notification::NotificationExt;
    app.notification().builder().title("PowerCost").body("Notifications work. You will see budget alerts like this.").show().map_err(|e| e.to_string())
}
