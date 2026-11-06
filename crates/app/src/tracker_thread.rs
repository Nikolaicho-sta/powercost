//! The background tracker thread: its own database connection, sampling every 10 s, publishing the
//! current reading, refreshing the tray tooltip, applying settings changes live, compacting old
//! detail, and sending monthly-budget notifications.

use crate::state::{Live, Shared};
use crate::update_tray;
use chrono::{Duration, Utc};
use cost_engine::TariffSet;
use dashboard::PeriodKind;
use estimator::Estimator;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use storage::Storage;
use tariff_engine::Tariff;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;
use tracker::{Tracker, TrackerConfig};

fn money_text(minor: i64, digits: u32, currency: &str) -> String {
    let unit = 10i64.pow(digits);
    format!("{}.{:0w$} {}", minor / unit, (minor % unit).abs(), currency, w = digits as usize)
}

fn tooltip(store: &Storage, tariff: &Tariff, tracker: &Tracker) -> String {
    let now = Utc::now();
    let range = dashboard::range_for(PeriodKind::Today, now, tariff.tz());
    let cost = store.summary(range.from, now).map(|s| money_text(s.total.cost_minor(2), 2, &tariff.currency)).unwrap_or_else(|_| "?".into());
    match tracker.current() {
        Some(c) => format!("PowerCost - today ~{cost} - now ~{} W ({})", c.watts.split('.').next().unwrap_or("?"), c.layer),
        None => format!("PowerCost - today ~{cost}"),
    }
}

pub fn spawn(app: AppHandle, db_path: std::path::PathBuf, live: Arc<Mutex<Live>>, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("powercost-tracker".into())
        .spawn(move || {
            let run = || -> Result<(), String> {
                let store = Storage::open(&db_path).map_err(|e| e.to_string())?;
                let reader = Storage::open(&db_path).map_err(|e| e.to_string())?;
                let (mut settings, mut tariff, mut versions, mut seen) = {
                    let l = live.lock().expect("live");
                    (l.settings.clone(), l.tariff.clone(), l.versions.clone(), l.version)
                };
                let est = Estimator::new(settings.estimator.clone()).map_err(|e| e.to_string())?;
                #[cfg(windows)]
                let (hub, events): (sensors::SensorHub, Box<dyn gap_classifier::EventSource + Send>) =
                    (sensors::SensorHub::detect(), Box::new(gap_classifier::wevtutil::WevtutilSource));
                #[cfg(not(windows))]
                let (hub, events): (sensors::SensorHub, Box<dyn gap_classifier::EventSource + Send>) = (sensors::SensorHub::default(), Box::new(NoEvents));

                let cfg = TrackerConfig { assumptions: settings.gap_assumptions.to_classifier(), thresholds: settings.thresholds, ..TrackerConfig::default() };
                let session = format!("{:x}-{:x}", Utc::now().timestamp_nanos_opt().unwrap_or_default(), std::process::id());
                let mut tr = Tracker::new(store, hub, est, TariffSet::new(versions.clone()).map_err(|e| e.to_string())?, events, cfg, session);

                let rep = tr.startup(Utc::now()).map_err(|e| e.to_string())?;
                if let Some(g) = &rep.gap {
                    eprintln!("recovered a {} s gap into {} spans", (g.end - g.start).num_seconds(), rep.spans_added);
                }
                shared.lock().expect("shared").tracker_running = true;

                let mut last_tip = Utc::now() - Duration::minutes(5);
                let mut last_budget = Utc::now() - Duration::minutes(5);
                let mut last_compact = Utc::now() - Duration::days(1);
                while !stop.load(Ordering::SeqCst) {
                    let now = Utc::now();

                    // apply changed settings / tariffs without a restart
                    {
                        let l = live.lock().expect("live");
                        if l.version != seen {
                            seen = l.version;
                            settings = l.settings.clone();
                            tariff = l.tariff.clone();
                            versions = l.versions.clone();
                            if let Ok(e) = Estimator::new(settings.estimator.clone()) {
                                tr.set_estimator(e);
                            }
                            if let Ok(set) = TariffSet::new(versions.clone()) {
                                tr.set_tariffs(set);
                            }
                            tr.set_thresholds(settings.thresholds);
                            tr.set_assumptions(settings.gap_assumptions.to_classifier());
                        }
                    }

                    if let Err(e) = tr.tick(now) {
                        eprintln!("tracker tick failed: {e}");
                    }
                    {
                        let mut s = shared.lock().expect("shared");
                        s.current = tr.current().cloned();
                        s.issues = tr.sensor_issues().iter().map(|i| format!("{}: {}", i.sensor, i.message)).collect();
                    }
                    if now - last_tip >= Duration::seconds(30) {
                        update_tray(&app, &tooltip(&reader, &tariff, &tr));
                        last_tip = now;
                    }
                    if now - last_budget >= Duration::seconds(60) {
                        last_budget = now;
                        match backend::check_budget(&reader, &settings, &tariff, now) {
                            Ok(Some(alert)) => {
                                if let Err(e) = app.notification().builder().title(&alert.title).body(&alert.body).show() {
                                    eprintln!("notification failed: {e}");
                                }
                            }
                            Ok(None) => {}
                            Err(e) => eprintln!("budget check failed: {e}"),
                        }
                    }
                    if now - last_compact >= Duration::hours(6) {
                        last_compact = now;
                        match backend::compact_if_due(tr.store_mut(), &settings, tariff.tz(), now) {
                            Ok(r) if r.minute_rows_removed > 0 => eprintln!("compacted {} minute rows", r.minute_rows_removed),
                            Ok(_) => {}
                            Err(e) => eprintln!("compaction failed: {e}"),
                        }
                        let _ = reader.prune_heartbeats(now - Duration::days(2));
                    }

                    // sleep ~10 s in short slices so quitting is prompt
                    for _ in 0..40 {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                }
                tr.shutdown(Utc::now()).map_err(|e| e.to_string())?;
                shared.lock().expect("shared").tracker_running = false;
                Ok(())
            };
            if let Err(e) = run() {
                eprintln!("tracker stopped: {e}");
                shared.lock().expect("shared").issues.push(format!("tracker: {e}"));
            }
        })
        .expect("spawn tracker thread")
}

#[cfg(not(windows))]
struct NoEvents;
#[cfg(not(windows))]
impl gap_classifier::EventSource for NoEvents {
    fn events(&self, _: chrono::DateTime<Utc>, _: chrono::DateTime<Utc>) -> Result<Vec<gap_classifier::PowerEvent>, String> {
        Ok(Vec::new())
    }
}
