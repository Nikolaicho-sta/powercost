//! export-mock <out-dir> [tariff.json]: writes the dashboard JSON the UI would receive, computed
//! from simulated data, so the frontend can be developed and previewed in a plain browser.

use chrono::{Duration, DurationRound, Utc};
use cost_engine::TariffSet;
use dashboard::{dashboard_at, what_if, CurrentInfo, DashOptions, PeriodKind, WhatIfParams};
use estimator::{Estimator, EstimatorConfig, Hardware, Inputs};
use pc_core::PowerState;
use rust_decimal::Decimal;
use simulator::{generate, SimConfig};
use std::path::PathBuf;
use storage::Storage;
use tariff_engine::Tariff;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).expect("usage: export-mock <out-dir> [tariff.json]"));
    let tariff_path = args.get(2).cloned().unwrap_or_else(|| "tariffs/bg-electrohold.json".into());
    std::fs::create_dir_all(&out).unwrap();

    let tariff = Tariff::from_json(&std::fs::read_to_string(tariff_path).unwrap()).unwrap();
    let tz = tariff.tz();
    let est = Estimator::new(EstimatorConfig {
        psu_efficiency: Decimal::new(90, 2),
        baseline_w: Decimal::from(8),
        hardware: Hardware { gpu_tdp_w: Some(Decimal::from(60)), gpu_idle_w: Decimal::from(5), cpu_tdp_w: Decimal::from(45), ..Hardware::default() },
        ..EstimatorConfig::default()
    })
    .unwrap();

    let now = Utc::now().duration_trunc(Duration::minutes(1)).unwrap();
    let jan1 = dashboard::range_for(PeriodKind::Year, now, tz).from;
    let spans = generate(&SimConfig::default(), &est, jan1, now, tz);
    let mut store = Storage::open_in_memory().unwrap();
    store.record_spans(&spans, &TariffSet::single(tariff.clone())).unwrap();
    store.compact(now - Duration::days(90), tz).unwrap(); // exercise the aggregate path too

    let current = CurrentInfo::new(
        PowerState::Active,
        &est.estimate(&Inputs { cpu_power_w: Some(18.0), gpu_power_w: Some(35.0), cpu_load_pct: 55.0, gpu_util_pct: 30.0, ..Inputs::new(PowerState::Active) }),
        Some(true),
    );

    for (name, kind) in [("today", PeriodKind::Today), ("week", PeriodKind::Week), ("month", PeriodKind::Month), ("year", PeriodKind::Year)] {
        // the current period plus a few earlier ones for the preview's back/forward buttons
        let earlier = if kind == PeriodKind::Today { 14 } else { 3 };
        for back in 0..=earlier {
            let d = dashboard_at(&store, &tariff, kind, now, -back, Some(current.clone()), &DashOptions::default()).unwrap();
            let file = if back == 0 { format!("dashboard-{name}.json") } else { format!("dashboard-{name}-{back}.json") };
            std::fs::write(out.join(file), serde_json::to_string_pretty(&d).unwrap()).unwrap();
        }
    }
    let (from, to) = (now - Duration::days(30), now);
    for shift in [0u32, 25, 50, 75, 100] {
        let p = WhatIfParams { shift_percent: shift, ..WhatIfParams::default() };
        let r = what_if(&store, &tariff, &p, from, to).unwrap();
        std::fs::write(out.join(format!("whatif-{shift}.json")), serde_json::to_string_pretty(&r).unwrap()).unwrap();
    }
    // tariff catalogue for the editor preview
    let mut catalog = tariff_engine::Catalog::new();
    for f in ["bg-electrohold.json", "bg-evn.json", "bg-energo-pro.json", "examples/de-flat.json", "examples/uk-economy7.json"] {
        let path = PathBuf::from("tariffs").join(f);
        catalog.add_bundled(f, &std::fs::read_to_string(path).unwrap());
    }
    let list = serde_json::json!({ "presets": catalog.list(), "problems": catalog.problems, "selected_id": "bg-electrohold", "user_dir": "(preview)" });
    std::fs::write(out.join("tariffs.json"), serde_json::to_string_pretty(&list).unwrap()).unwrap();
    for p in catalog.list() {
        let t = catalog.get(&p.id, p.version).unwrap();
        std::fs::write(out.join(format!("tariff-{}-{}.json", p.id, p.version)), serde_json::to_string_pretty(t).unwrap()).unwrap();
    }
    println!("wrote mock data to {}", out.display());
}
