//! Every shipped preset must load and validate; Electrohold values are pinned to the
//! official page (https://electrohold.bg/bg/sales/domakinstva/snabdyavane-po-regulirani-ceni/).

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use std::{fs, path::PathBuf, str::FromStr};
use tariff_engine::Tariff;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tariffs")
}

fn json_files(dir: PathBuf) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect()
}

fn load(name: &str) -> Tariff {
    Tariff::from_json(&fs::read_to_string(root().join(name)).unwrap()).unwrap()
}

fn sofia(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    chrono_tz::Europe::Sofia.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().with_timezone(&Utc)
}

#[test]
fn all_shipped_presets_validate() {
    let mut files = json_files(root());
    files.extend(json_files(root().join("examples")));
    assert!(files.len() >= 5, "expected BG x3 + 2 examples, found {}", files.len());
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        Tariff::from_json(&text).unwrap_or_else(|e| panic!("{} failed: {e}", f.display()));
    }
}

#[test]
fn electrohold_prices_match_the_official_page() {
    let t = load("bg-electrohold.json");
    let d = |s| Decimal::from_str(s).unwrap();
    // Official "final prices incl. network services" column, excl. VAT.
    assert_eq!(t.price("day").unwrap().excl_vat, d("0.12854"));
    assert_eq!(t.price("night").unwrap().excl_vat, d("0.07660"));
    // With 20% VAT.
    assert_eq!(t.price("day").unwrap().gross, d("0.154248"));
    assert_eq!(t.price("night").unwrap().gross, d("0.091920"));
}

#[test]
fn electrohold_hours_follow_calendar_dates_not_dst() {
    let t = load("bg-electrohold.json");
    let p = |when| t.resolve(when).period_id.to_string();
    // winter 01.11-31.03: night 22-06
    assert_eq!(p(sofia(2026, 11, 6, 5, 59)), "night");
    assert_eq!(p(sofia(2026, 11, 6, 6, 0)), "day");
    assert_eq!(p(sofia(2026, 11, 6, 21, 59)), "day");
    assert_eq!(p(sofia(2026, 11, 6, 22, 0)), "night");
    // summer 01.04-31.10: night 23-07
    assert_eq!(p(sofia(2026, 4, 1, 6, 59)), "night");
    assert_eq!(p(sofia(2026, 4, 1, 7, 0)), "day");
    assert_eq!(p(sofia(2026, 4, 1, 22, 59)), "day");
    assert_eq!(p(sofia(2026, 4, 1, 23, 0)), "night");
    // 29.03.2026 is the DST change but still "winter" hours by calendar date
    assert_eq!(p(sofia(2026, 3, 29, 22, 30)), "night");
    assert_eq!(t.resolve(sofia(2026, 3, 29, 22, 30)).season_id, "winter");
    // weekends are not special
    assert_eq!(p(sofia(2026, 11, 7, 12, 0)), "day");
}

#[test]
fn unverified_presets_are_flagged() {
    use tariff_engine::Warning;
    let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
    let evn = load("bg-evn.json");
    assert!(evn.warnings(today).contains(&Warning::MarkedUnverified));
    assert!(evn.warnings(today).contains(&Warning::NeverVerified));
    assert!(!load("bg-electrohold.json").warnings(today).contains(&Warning::MarkedUnverified));
}
