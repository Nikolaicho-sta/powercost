//! Tariff catalogue: bundled presets + user files, grouped by id with several versions
//! (prices change, e.g. every 1 July). Countries are added by dropping a JSON file into the
//! user tariff folder; no code change needed.

use crate::{Tariff, TariffError, Verification};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetSource {
    Bundled,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetInfo {
    pub id: String,
    pub version: u32,
    pub name: String,
    pub country: String,
    pub distributor: Option<String>,
    pub currency: String,
    pub verification: Option<Verification>,
    pub valid_from: NaiveDate,
    pub valid_to: Option<NaiveDate>,
    pub source: PresetSource,
}

struct Entry {
    source: PresetSource,
    tariff: Tariff,
}

#[derive(Default)]
pub struct Catalog {
    entries: Vec<Entry>,
    /// Files that could not be loaded (shown to the user, never fatal).
    pub problems: Vec<String>,
}

fn info(e: &Entry) -> PresetInfo {
    let t = &e.tariff;
    PresetInfo {
        id: t.id.clone(),
        version: t.version,
        name: t.name.clone(),
        country: t.country.clone(),
        distributor: t.distributor.clone(),
        currency: t.currency.clone(),
        verification: t.verification,
        valid_from: t.valid_from,
        valid_to: t.valid_to,
        source: e.source,
    }
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, source: PresetSource, tariff: Tariff) {
        // the same (id, version) can only exist once; a later add (user file) replaces it
        self.entries.retain(|e| !(e.tariff.id == tariff.id && e.tariff.version == tariff.version));
        self.entries.push(Entry { source, tariff });
    }

    pub fn add_bundled(&mut self, name: &str, json: &str) {
        match Tariff::from_json(json) {
            Ok(t) => self.insert(PresetSource::Bundled, t),
            Err(e) => self.problems.push(format!("bundled preset {name}: {e}")),
        }
    }

    /// Load every `*.json` in `dir` (missing directory = nothing to load).
    pub fn load_user_dir(&mut self, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        for f in files {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|s| Tariff::from_json(s.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())) {
                Ok(t) => self.insert(PresetSource::User, t),
                Err(e) => self.problems.push(format!("{name}: {e}")),
            }
        }
    }

    /// Every stored version, sorted by country, name, version.
    pub fn list(&self) -> Vec<PresetInfo> {
        let mut v: Vec<PresetInfo> = self.entries.iter().map(info).collect();
        v.sort_by(|a, b| (&a.country, &a.name, a.version).cmp(&(&b.country, &b.name, b.version)));
        v
    }

    /// Latest version of each tariff id.
    pub fn latest(&self) -> Vec<PresetInfo> {
        let mut out: Vec<PresetInfo> = Vec::new();
        for i in self.list() {
            match out.iter_mut().find(|o| o.id == i.id) {
                Some(o) if i.valid_from >= o.valid_from => *o = i,
                Some(_) => {}
                None => out.push(i),
            }
        }
        out
    }

    pub fn for_country(&self, country: &str) -> Vec<PresetInfo> {
        self.latest().into_iter().filter(|p| p.country.eq_ignore_ascii_case(country)).collect()
    }

    /// All versions of `id`, oldest first - the set used for pricing history.
    pub fn versions(&self, id: &str) -> Vec<Tariff> {
        let mut v: Vec<Tariff> = self.entries.iter().filter(|e| e.tariff.id == id).map(|e| e.tariff.clone()).collect();
        v.sort_by_key(|t| (t.valid_from, t.version));
        v
    }

    pub fn get(&self, id: &str, version: u32) -> Option<&Tariff> {
        self.entries.iter().find(|e| e.tariff.id == id && e.tariff.version == version).map(|e| &e.tariff)
    }

    pub fn is_user(&self, id: &str, version: u32) -> bool {
        self.entries.iter().any(|e| e.tariff.id == id && e.tariff.version == version && e.source == PresetSource::User)
    }

    /// The version in force on `date` (latest `valid_from` <= date, else the earliest).
    pub fn current(&self, id: &str, date: NaiveDate) -> Option<Tariff> {
        let v = self.versions(id);
        v.iter().rfind(|t| t.valid_from <= date).or(v.first()).cloned()
    }

    /// Validate and store a user tariff as `<dir>/<id>.v<version>.json`, then add it.
    pub fn save_user(&mut self, dir: &Path, tariff_json: serde_json::Value) -> Result<Tariff, TariffError> {
        let t = Tariff::from_value(tariff_json)?;
        if let Some(existing) = self.get(&t.id, t.version) {
            if !self.is_user(&existing.id, existing.version) {
                return Err(TariffError::Invalid(format!("'{}' version {} is a bundled preset and cannot be overwritten; copy it under a new id", t.id, t.version)));
            }
        }
        std::fs::create_dir_all(dir).map_err(|e| TariffError::Invalid(format!("cannot create {}: {e}", dir.display())))?;
        let path = dir.join(format!("{}.v{}.json", t.id, t.version));
        let text = serde_json::to_string_pretty(&t)?;
        std::fs::write(&path, text).map_err(|e| TariffError::Invalid(format!("cannot write {}: {e}", path.display())))?;
        self.insert(PresetSource::User, t.clone());
        Ok(t)
    }

    pub fn delete_user(&mut self, dir: &Path, id: &str, version: u32) -> Result<(), String> {
        if !self.is_user(id, version) {
            return Err("only your own tariffs can be deleted".into());
        }
        let _ = std::fs::remove_file(dir.join(format!("{id}.v{version}.json")));
        self.entries.retain(|e| !(e.tariff.id == id && e.tariff.version == version));
        Ok(())
    }
}