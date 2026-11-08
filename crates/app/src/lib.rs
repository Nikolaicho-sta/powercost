//! PowerCost desktop shell: tray icon, window lifecycle, commands for the UI and the background
//! tracker thread. Local-first: no network access anywhere in this crate (the optional IP region
//! lookup is made by the UI only after the user opts in).
//!
//! Memory design: the tracker runs in this process; the WebView window is DESTROYED (not hidden)
//! when closed, so the resident cost is the tracker only. Click the tray icon to recreate it.

mod commands;
mod state;
mod tracker_thread;
mod online;
mod updater;

use chrono::Utc;
use cost_engine::TariffSet;
use dashboard::{CurrentInfo, PeriodKind};
use estimator::{Estimator, EstimatorConfig};
use settings::Settings;
use state::{build_catalog, AppState, Live, Shared};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use storage::Storage;
use tariff_engine::Tariff;
use tauri::menu::MenuBuilder;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

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
    // An empty variable would turn the data folder into a relative path in the current directory.
    let non_empty = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    let base = non_empty("APPDATA").or_else(|| non_empty("HOME")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
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
    // a demo database skips the first-run wizard
    Settings { first_run_done: true, ..Settings::default() }.save(&store).map_err(|e| e.join("; "))?;
    Ok(path)
}

pub(crate) fn update_tray(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(text));
    }
}

/// Background update check; shows ONE Windows notification per new update.
/// * Folder mode: a local `git log` about a minute after start, then every 6 hours. No network.
/// * GitHub mode: about once a day while "look for updates automatically" is on in Settings (on by
///   default, it is a network call and can be switched off). Pressing "Check for updates" always works.
fn spawn_update_checker(app: AppHandle) {
    use tauri_plugin_notification::NotificationExt;
    let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
        let stopped = || app.state::<AppState>().stop.load(Ordering::SeqCst);
        // wake once a minute-ish; decide what is due
        let sleep = |secs: u64| {
            for _ in 0..secs / 5 {
                if stopped() {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
            true
        };
        if !sleep(60) {
            return;
        }
        let mut last: Option<std::time::Instant> = None;
        loop {
            let state = app.state::<AppState>();
            let (every, wanted) = match updater::mode() {
                updater::UpdateSource::Folder => (6 * 3600, true),
                updater::UpdateSource::Github => (24 * 3600, state.live.lock().unwrap_or_else(|e| e.into_inner()).settings.online_update_check),
            };
            let due = wanted && last.is_none_or(|t| t.elapsed().as_secs() >= every);
            if due {
                last = Some(std::time::Instant::now());
                let status = updater::check();
                *state.update.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
                if let updater::UpdateStatus::Available { id, label, subject, .. } = status {
                    let marker = state.data_dir.join("update_notified.txt");
                    if std::fs::read_to_string(&marker).map(|s| s.trim() != id).unwrap_or(true) {
                        let body = format!("{label}: {subject} Open PowerCost, Settings, and press \"Update now\".");
                        if app.notification().builder().title("PowerCost update available").body(body).show().is_ok() {
                            let _ = std::fs::write(&marker, &id);
                        }
                    }
                }
            }
            if !sleep(600) {
                return;
            }
        }
    });
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let simulated = args.iter().any(|a| a == "--sim");
    let start_in_tray = args.iter().any(|a| a == "--tray");
    // `--quit` only exists to stop a running instance. If no instance runs, this process must exit
    // at once and must not touch the real data folder.
    let quit_only = args.iter().any(|a| a == "--quit");

    let dir = if quit_only { std::env::temp_dir().join("PowerCost-quit") } else { data_dir() };
    std::fs::create_dir_all(&dir).expect("data directory");
    let catalog = build_catalog(&dir);
    let default_tariff = catalog.current(&Settings::default().tariff_id, Utc::now().date_naive()).expect("bundled default tariff");

    let db_path = if simulated { prepare_simulation(&default_tariff).expect("simulation database") } else { dir.join("powercost.db") };
    let settings = {
        let store = Storage::open(&db_path).expect("database");
        Settings::load(&store).unwrap_or_default()
    };
    let live = Live::resolve(&catalog, settings, 1).expect("an available tariff");

    let state = AppState {
        db_path,
        data_dir: dir,
        simulated,
        catalog: Mutex::new(catalog),
        live: Arc::new(Mutex::new(live)),
        shared: Arc::new(Mutex::new(Shared::default())),
        stop: Arc::new(AtomicBool::new(false)),
        tracker: Mutex::new(None),
        update: Mutex::new(updater::UpdateStatus::default()),
    };

    let mut builder = tauri::Builder::default();
    if !simulated {
        // Must be the first plugin. A second launch (login autostart plus a manual start, or a
        // double-click on the shortcut) opens the existing instance's window instead of starting
        // a second tracker on the same database.
        // `powercost.exe --quit` from a second process asks THIS instance to exit cleanly (the update
        // script uses it so the last minute is saved instead of the process being killed).
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.iter().any(|a| a == "--quit") {
                app.exit(0);
            } else {
                open_main_window(app);
            }
        }));
    }
    let app = builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_dashboard,
            commands::get_what_if,
            commands::get_app_info,
            commands::get_update_status,
            commands::check_update,
            commands::start_update,
            commands::get_settings,
            commands::save_settings,
            commands::list_tariffs,
            commands::get_tariff,
            commands::validate_tariff,
            commands::save_tariff,
            commands::delete_tariff,
            commands::detect_region,
            commands::presets_for_country,
            commands::save_calibration,
            commands::reprice_history,
            commands::export_data,
            commands::send_test_notification,
            commands::stopwatch_start,
            commands::stopwatch_status,
            commands::stopwatch_stop,
            commands::stopwatch_cancel,
            commands::stopwatch_history,
            commands::stopwatch_delete,
        ])
        .setup(move |app| {
            if quit_only {
                app.handle().exit(0);
                return Ok(());
            }
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
                let handle = tracker_thread::spawn(app.handle().clone(), state.db_path.clone(), state.live.clone(), state.shared.clone(), state.stop.clone());
                *state.tracker.lock().expect("tracker handle") = Some(handle);
                spawn_update_checker(app.handle().clone());
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
