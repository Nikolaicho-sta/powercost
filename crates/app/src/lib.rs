//! PowerCost desktop shell: tray icon, window lifecycle, read-only commands for the UI and the
//! background tracker thread. Local-first: no network access anywhere in this crate.
//!
//! Memory design: the tracker runs in this process; the WebView window is DESTROYED (not hidden)
//! when closed, so the resident cost is the tracker only. Click the tray icon to recreate it.

mod tracker_thread;

use chrono::Utc;
use cost_engine::TariffSet;
use dashboard::{CurrentInfo, Dashboard, PeriodKind, WhatIfParams, WhatIfResult};
use estimator::{Estimator, EstimatorConfig};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use storage::Storage;
use tariff_engine::Tariff;
use tauri::menu::MenuBuilder;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};

/// Tariff presets are data files compiled in; the tariff editor (M5) will add user copies.
const DEFAULT_TARIFF: &str = include_str!("../../../tariffs/bg-electrohold.json");

#[derive(Default)]
pub struct Shared {
    pub current: Option<CurrentInfo>,
    pub issues: Vec<String>,
    pub tracker_running: bool,
}

pub struct AppState {
    pub db_path: PathBuf,
    pub simulated: bool,
    pub tariff: Tariff,
    pub shared: Arc<Mutex<Shared>>,
    pub stop: Arc<AtomicBool>,
    pub tracker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[derive(Serialize)]
struct AppInfo {
    simulated: bool,
    preview: bool,
    version: &'static str,
    tracker_running: bool,
    sensor_issues: Vec<String>,
}

fn open(state: &AppState) -> Result<Storage, String> {
    Storage::open(&state.db_path).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_dashboard(state: State<AppState>, period: PeriodKind) -> Result<Dashboard, String> {
    let store = open(&state)?;
    let current = state.shared.lock().map_err(|e| e.to_string())?.current.clone();
    dashboard::dashboard(&store, &state.tariff, period, Utc::now(), current).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_what_if(state: State<AppState>, params: WhatIfParams, days: u32) -> Result<WhatIfResult, String> {
    let store = open(&state)?;
    let to = Utc::now();
    dashboard::what_if(&store, &state.tariff, &params, to - chrono::Duration::days(days.clamp(1, 90) as i64), to).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_app_info(state: State<AppState>) -> AppInfo {
    let shared = state.shared.lock().expect("shared state");
    AppInfo {
        simulated: state.simulated,
        preview: false,
        version: env!("CARGO_PKG_VERSION"),
        tracker_running: shared.tracker_running,
        sensor_issues: shared.issues.clone(),
    }
}

fn open_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("PowerCost")
        .inner_size(1100.0, 900.0)
        .min_inner_size(480.0, 520.0)
        .build();
    if let Err(e) = built {
        eprintln!("could not open the window: {e}");
    }
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("PowerCost")
}

/// Fill (or reuse) a simulated database so the UI can be tried without waiting for real data.
fn prepare_simulation(tariff: &Tariff) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join("powercost-sim.db");
    let fresh = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 12 * 3600);
    if fresh {
        return Ok(path);
    }
    let _ = std::fs::remove_file(&path);
    let est = Estimator::new(EstimatorConfig::default()).map_err(|e| e.to_string())?;
    let now = Utc::now();
    let tz = tariff.tz();
    let from = dashboard::range_for(PeriodKind::Year, now, tz).from;
    let spans = simulator::generate(&simulator::SimConfig::default(), &est, from, now, tz);
    let mut store = Storage::open(&path).map_err(|e| e.to_string())?;
    store.record_spans(&spans, &TariffSet::single(tariff.clone())).map_err(|e| e.to_string())?;
    store.compact(now - chrono::Duration::days(90), tz).map_err(|e| e.to_string())?;
    Ok(path)
}

fn update_tray(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(text));
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let simulated = args.iter().any(|a| a == "--sim");
    let start_in_tray = args.iter().any(|a| a == "--tray");

    let tariff = Tariff::from_json(DEFAULT_TARIFF).expect("embedded tariff is valid");
    let db_path = if simulated {
        prepare_simulation(&tariff).expect("simulation database")
    } else {
        let dir = data_dir();
        std::fs::create_dir_all(&dir).expect("data directory");
        dir.join("powercost.db")
    };

    let shared = Arc::new(Mutex::new(Shared::default()));
    let state = AppState { db_path, simulated, tariff, shared, stop: Arc::new(AtomicBool::new(false)), tracker: Mutex::new(None) };

    let app = tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![get_dashboard, get_what_if, get_app_info])
        .setup(move |app| {
            let menu = MenuBuilder::new(app).text("open", "Open dashboard").separator().text("quit", "Quit PowerCost").build()?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("window icon"))
                .tooltip("PowerCost")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => open_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        open_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            let state = app.state::<AppState>();
            if state.simulated {
                // No live tracker in simulation mode; show a representative "now" and a static tooltip.
                if let Ok(est) = Estimator::new(EstimatorConfig::default()) {
                    let inputs = estimator::Inputs { cpu_load_pct: 40.0, ..estimator::Inputs::new(pc_core::PowerState::Active) };
                    state.shared.lock().expect("shared").current = Some(CurrentInfo::new(pc_core::PowerState::Active, &est.estimate(&inputs), Some(true)));
                }
                update_tray(app.handle(), "PowerCost (simulated data)");
            } else {
                let handle = tracker_thread::spawn(app.handle().clone(), state.db_path.clone(), state.tariff.clone(), state.shared.clone(), state.stop.clone());
                *state.tracker.lock().expect("tracker handle") = Some(handle);
            }

            if !start_in_tray {
                open_main_window(app.handle());
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building PowerCost");

    app.run(|app, event| match event {
        // Closing the window must not quit: the tracker keeps running in the tray.
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        RunEvent::Exit => {
            let state = app.state::<AppState>();
            state.stop.store(true, Ordering::SeqCst);
            let handle = state.tracker.lock().expect("tracker handle").take();
            if let Some(h) = handle {
                let _ = h.join(); // flushes the minute in progress and writes a final heartbeat
            }
        }
        _ => {}
    });
}