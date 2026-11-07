//! Shared application state. `Live` is what the tracker thread needs; it carries a version so the
//! thread notices when the user changed a setting or tariff and applies it without a restart.

use backend::{active_tariff, ActiveTariff};
use chrono::Utc;
use dashboard::CurrentInfo;
use settings::Settings;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tariff_engine::{Catalog, Tariff};

/// Bundled tariff presets (data files compiled in). Users add more by dropping JSON files into
/// `<data dir>/tariffs`.
pub const BUNDLED: &[(&str, &str)] = &[
    ("bg-electrohold.json", include_str!("../../../tariffs/bg-electrohold.json")),
    ("bg-evn.json", include_str!("../../../tariffs/bg-evn.json")),
    ("bg-energo-pro.json", include_str!("../../../tariffs/bg-energo-pro.json")),
    ("de-flat.json", include_str!("../../../tariffs/examples/de-flat.json")),
    ("uk-economy7.json", include_str!("../../../tariffs/examples/uk-economy7.json")),
];

#[derive(Default)]
pub struct Shared {
    pub current: Option<CurrentInfo>,
    pub issues: Vec<String>,
    pub tracker_running: bool,
}

/// Settings + resolved tariff versions; `version` increases on every change.
pub struct Live {
    pub settings: Settings,
    pub tariff: Tariff,
    pub versions: Vec<Tariff>,
    pub tariff_note: Option<String>,
    pub version: u64,
}

impl Live {
    pub fn resolve(catalog: &Catalog, settings: Settings, version: u64) -> Result<Live, String> {
        let ActiveTariff { current, set: _, fallback_note } = active_tariff(catalog, &settings, Utc::now().date_naive()).map_err(|e| e.to_string())?;
        let versions = catalog.versions(&current.id);
        Ok(Live { settings, tariff: current, versions, tariff_note: fallback_note, version })
    }
}

pub struct AppState {
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
    pub simulated: bool,
    pub catalog: Mutex<Catalog>,
    pub live: Arc<Mutex<Live>>,
    pub shared: Arc<Mutex<Shared>>,
    pub stop: Arc<AtomicBool>,
    pub tracker: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub update: Mutex<crate::updater::UpdateStatus>,
}

impl AppState {
    pub fn tariff_dir(&self) -> PathBuf {
        self.data_dir.join("tariffs")
    }
}

pub fn build_catalog(data_dir: &std::path::Path) -> Catalog {
    let mut c = Catalog::new();
    for (name, json) in BUNDLED {
        c.add_bundled(name, json);
    }
    c.load_user_dir(&data_dir.join("tariffs"));
    c
}
