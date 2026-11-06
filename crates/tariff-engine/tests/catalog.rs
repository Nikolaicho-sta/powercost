//! Catalogue, region guess and serialisation round-trip tests.

use chrono::NaiveDate;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use tariff_engine::*;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tariffs")
}

fn bundled(c: &mut Catalog) {
    for f in ["bg-electrohold.json", "bg-evn.json", "bg-energo-pro.json", "examples/de-flat.json", "examples/uk-economy7.json"] {
        c.add_bundled(f, &fs::read_to_string(root().join(f)).unwrap());
    }
}

fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, dd).unwrap()
}

fn user_tariff(id: &str, version: u32, valid_from: &str, price: &str) -> Value {
    serde_json::json!({
        "schema_version": 1, "id": id, "version": version, "name": "My tariff", "country": "BG", "currency": "EUR",
        "timezone": "Europe/Sofia", "vat_percent": "20", "valid_from": valid_from,
        "components": [{"id": "all", "kind": "energy", "vat_inclusive": false, "per_kwh": price}],
        "periods": [{"id": "flat", "label": "Flat"}],
        "schedule": {"seasons": [{"id": "all", "from": "01-01", "to": "12-31", "rules": [
            {"days": ["all"], "ranges": [{"start": "00:00", "end": "24:00"}], "period": "flat"}]}]}
    })
}

// ---------- serialisation ----------

#[test]
fn every_preset_survives_a_serialise_and_reload_round_trip() {
    let mut files: Vec<PathBuf> = fs::read_dir(root()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.extend(fs::read_dir(root().join("examples")).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "json")));
    assert!(files.len() >= 5);
    for f in files {
        let t = Tariff::from_json(&fs::read_to_string(&f).unwrap()).unwrap();
        let text = serde_json::to_string_pretty(&t).unwrap();
        let back = Tariff::from_json(&text).unwrap_or_else(|e| panic!("{} did not reload: {e}\n{text}", f.display()));
        assert_eq!(serde_json::to_value(&t).unwrap(), serde_json::to_value(&back).unwrap(), "{}", f.display());
        assert_eq!(t.price_for_index(0).gross, back.price_for_index(0).gross);
    }
}

// ---------- region ----------

#[test]
fn timezone_wins_over_locale_and_both_are_offered() {
    let g = guess_region("bg-BG", "Europe/Sofia");
    assert_eq!(g.candidates.len(), 1);
    assert_eq!((g.best(), g.candidates[0].source), (Some("BG"), GuessSource::Timezone));

    let g = guess_region("en-US", "Europe/Sofia");
    assert_eq!(g.candidates.iter().map(|c| c.country.as_str()).collect::<Vec<_>>(), ["BG", "US"]);
    assert_eq!(g.candidates[1].source, GuessSource::Locale);
}

#[test]
fn locale_only_and_unknown_cases() {
    assert_eq!(guess_region("de_DE", "").best(), Some("DE"));
    assert_eq!(guess_region("sr-Latn-RS", "Mars/Olympus").best(), Some("RS"));
    assert_eq!(guess_region("en-GB-u-ca-gregory", "").best(), Some("GB"));
    assert_eq!(guess_region("en", "Mars/Olympus").best(), None);
    assert!(guess_region("", "").candidates.is_empty());
    assert_eq!(guess_region("", "europe/sofia").best(), Some("BG"), "timezone match ignores case");
}

// ---------- catalogue ----------

#[test]
fn catalogue_lists_filters_and_groups_by_country() {
    let mut c = Catalog::new();
    bundled(&mut c);
    assert!(c.problems.is_empty(), "{:?}", c.problems);
    let bg: Vec<_> = c.for_country("bg").iter().map(|p| p.id.clone()).collect();
    assert_eq!(bg.len(), 3);
    assert!(bg.contains(&"bg-electrohold".to_string()));
    assert_eq!(c.for_country("DE").len(), 1);
    assert!(c.for_country("FR").is_empty());
    assert_eq!(c.list().len(), 5);
}

#[test]
fn versions_are_ordered_and_current_follows_the_date() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Catalog::new();
    c.save_user(dir.path(), user_tariff("user-home", 1, "2025-07-01", "0.10")).unwrap();
    c.save_user(dir.path(), user_tariff("user-home", 2, "2026-07-01", "0.12")).unwrap();
    let v = c.versions("user-home");
    assert_eq!(v.iter().map(|t| t.version).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(c.latest().iter().find(|p| p.id == "user-home").unwrap().version, 2);
    assert_eq!(c.current("user-home", d(2026, 6, 30)).unwrap().version, 1);
    assert_eq!(c.current("user-home", d(2026, 7, 1)).unwrap().version, 2);
    assert_eq!(c.current("user-home", d(2024, 1, 1)).unwrap().version, 1, "before the first version: the earliest");
    assert!(c.current("nope", d(2026, 1, 1)).is_none());
}

#[test]
fn saved_user_tariffs_are_files_that_reload_in_a_fresh_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Catalog::new();
    c.save_user(dir.path(), user_tariff("user-home", 1, "2026-01-01", "0.10")).unwrap();
    assert!(dir.path().join("user-home.v1.json").exists());

    let mut fresh = Catalog::new();
    fresh.load_user_dir(dir.path());
    assert!(fresh.problems.is_empty());
    let t = fresh.get("user-home", 1).unwrap();
    assert_eq!(t.price("flat").unwrap().excl_vat.to_string(), "0.10");
    assert!(fresh.is_user("user-home", 1));
}

#[test]
fn dropping_a_json_file_adds_a_country_and_bad_files_are_reported_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    // a hand-written file with a UTF-8 BOM (Windows editors add one)
    let mut fr = user_tariff("fr-base", 1, "2026-01-01", "0.20");
    fr["country"] = "FR".into();
    fr["timezone"] = "Europe/Paris".into();
    fs::write(dir.path().join("fr.json"), format!("\u{feff}{fr}")).unwrap();
    fs::write(dir.path().join("broken.json"), "{ not json").unwrap();
    fs::write(dir.path().join("invalid.json"), r#"{"schema_version": 1}"#).unwrap();
    fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

    let mut c = Catalog::new();
    bundled(&mut c);
    c.load_user_dir(dir.path());
    assert_eq!(c.for_country("FR").len(), 1);
    assert_eq!(c.problems.len(), 2);
    assert!(c.problems.iter().any(|p| p.starts_with("broken.json")));
    assert!(c.problems.iter().any(|p| p.starts_with("invalid.json")));
    assert_eq!(c.for_country("BG").len(), 3, "existing presets unaffected");
}

#[test]
fn bundled_presets_cannot_be_overwritten_but_can_be_copied() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Catalog::new();
    bundled(&mut c);
    let mut v: Value = serde_json::to_value(c.get("bg-electrohold", 2026).unwrap()).unwrap();
    let err = c.save_user(dir.path(), v.clone()).unwrap_err().to_string();
    assert!(err.contains("bundled"), "{err}");
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none(), "nothing written");

    v["id"] = "user-electrohold".into();
    v["version"] = 1.into();
    v["verification"] = "unverified".into();
    let saved = c.save_user(dir.path(), v).unwrap();
    assert_eq!(saved.id, "user-electrohold");
    assert!(c.is_user("user-electrohold", 1));
}

#[test]
fn invalid_tariffs_are_rejected_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Catalog::new();
    let mut bad = user_tariff("user-bad", 1, "2026-01-01", "0.10");
    bad["schedule"]["seasons"][0]["rules"][0]["ranges"] = serde_json::json!([{"start": "00:00", "end": "12:00"}]); // gap
    assert!(c.save_user(dir.path(), bad).is_err());
    let mut bad = user_tariff("../evil", 1, "2026-01-01", "0.10"); // path characters violate the id pattern
    bad["id"] = "../evil".into();
    assert!(c.save_user(dir.path(), bad).is_err());
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
}

#[test]
fn user_tariffs_can_be_deleted_bundled_cannot() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Catalog::new();
    bundled(&mut c);
    c.save_user(dir.path(), user_tariff("user-home", 1, "2026-01-01", "0.10")).unwrap();
    assert!(c.delete_user(dir.path(), "bg-electrohold", 2026).is_err());
    c.delete_user(dir.path(), "user-home", 1).unwrap();
    assert!(c.get("user-home", 1).is_none());
    assert!(!dir.path().join("user-home.v1.json").exists());
}