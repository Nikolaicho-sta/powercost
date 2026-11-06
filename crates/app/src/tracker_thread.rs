//! The background tracker thread: opens its own database connection, samples every 10 s,
//! publishes the current reading and refreshes the tray tooltip.

use crate::{update_tray, Shared};
use chrono::{Duration, Utc};
use cost_engine::TariffSet;
use dashboard::PeriodKind;
use estimator::{Estimator, EstimatorConfig};
use storage::Storage;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tariff_engine::Tariff;
use tauri::AppHandle;
use tracker::{Tracker, TrackerConfig};

fn money_text(minor: i64, digits: u32, currency: &str) -> String {
    format!("{:.*} {}", digits as usize, minor as f64 / 10f64.powi(digits as i32), currency)
}

fn tooltip(store: &Storage, tariff: &Tariff, tracker: &Tracker) -> String {
    let now = Utc::now();
    let range = dashboard::range_for(PeriodKind::Today, now, tariff.tz());
    let cost = store
        .summary(range.from, now)
        .map(|s| money_text(s.total.cost_minor(2), 2, &tariff.currency))
        .unwrap_or_else(|_| "?".into());
    match tracker.current() {
        Some(c) => format!("PowerCost - today ~{cost} - now ~{} W ({})", c.watts.split('.').next().unwrap_or("?"), c.layer),
        None => format!("PowerCost - today ~{cost}"),
    }
}

pub fn spawn(app: AppHandle, db_path: std::path::PathBuf, tariff: Tariff, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("powercost-tracker".into())
        .spawn(move || {
            let run = || -> Result<(), String> {
                let store = Storage::open(&db_path).map_err(|e| e.to_string())?;
                let reader = Storage::open(&db_path).map_err(|e| e.to_string())?;
                let est = Estimator::new(store.get_setting::<EstimatorConfig>("estimator").map_err(|e| e.to_string())?.unwrap_or_default())
                    .map_err(|e| e.to_string())?;
                #[cfg(windows)]
                let (hub, events): (sensors::SensorHub, Box<dyn gap_classifier::EventSource + Send>) =
                    (sensors::SensorHub::detect(), Box::new(gap_classifier::wevtutil::WevtutilSource));
                #[cfg(not(windows))]
                let (hub, events): (sensors::SensorHub, Box<dyn gap_classifier::EventSource + Send>) = (sensors::SensorHub::default(), Box::new(NoEvents));

                let session = format!("{:x}-{:x}", Utc::now().timestamp_nanos_opt().unwrap_or_default(), std::process::id());
                let mut tr = Tracker::new(store, hub, est, TariffSet::single(tariff.clone()), events, TrackerConfig::default(), session);

                let rep = tr.startup(Utc::now()).map_err(|e| e.to_string())?;
                if let Some(g) = &rep.gap {
                    eprintln!("recovered a {} s gap into {} spans", (g.end - g.start).num_seconds(), rep.spans_added);
                }
                shared.lock().expect("shared").tracker_running = true;

                let mut last_tip = Utc::now() - Duration::minutes(5);
                while !stop.load(Ordering::SeqCst) {
                    let now = Utc::now();
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