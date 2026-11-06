//! powercost-sim --days 60 --out sim.db [--seed 42] [--tariff tariffs/bg-electrohold.json]
//! Fills a database with simulated usage, compacts old history and prints a summary.

use chrono::{Duration, DurationRound, Utc};
use cost_engine::TariffSet;
use estimator::{Estimator, EstimatorConfig, Hardware};
use rust_decimal::Decimal;
use simulator::{generate, SimConfig};
use std::{path::PathBuf, time::Instant};
use storage::Storage;
use tariff_engine::Tariff;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let days: i64 = arg(&args, "--days").and_then(|v| v.parse().ok()).unwrap_or(60);
    let out = PathBuf::from(arg(&args, "--out").unwrap_or_else(|| "sim.db".into()));
    let seed: u64 = arg(&args, "--seed").and_then(|v| v.parse().ok()).unwrap_or(42);
    let tariff_path = arg(&args, "--tariff").unwrap_or_else(|| "tariffs/bg-electrohold.json".into());
    let keep_days: i64 = arg(&args, "--keep-days").and_then(|v| v.parse().ok()).unwrap_or(90);

    let tariff = Tariff::from_json(&std::fs::read_to_string(&tariff_path).expect("tariff file")).expect("valid tariff");
    let tz = tariff.tz();
    let tariffs = TariffSet::single(tariff);

    let cfg = EstimatorConfig {
        psu_efficiency: Decimal::new(90, 2),
        baseline_w: Decimal::from(8),
        hardware: Hardware { gpu_tdp_w: Some(Decimal::from(60)), gpu_idle_w: Decimal::from(5), cpu_tdp_w: Decimal::from(45), ..Hardware::default() },
        ..EstimatorConfig::default()
    };
    let est = Estimator::new(cfg).expect("config");

    let _ = std::fs::remove_file(&out);
    let mut store = Storage::open(&out).expect("open db");
    let to = Utc::now().duration_trunc(Duration::minutes(1)).unwrap();
    let from = to - Duration::days(days);

    let t0 = Instant::now();
    let spans = generate(&SimConfig { seed, ..SimConfig::default() }, &est, from, to, tz);
    let t_gen = t0.elapsed();
    let t1 = Instant::now();
    store.record_spans(&spans, &tariffs).expect("store");
    let t_store = t1.elapsed();
    let rep = store.compact(to - Duration::days(keep_days), tz).expect("compact");
    let size = store.db_size_bytes().unwrap_or(0);

    let sum = store.summary(from, to).unwrap();
    println!("simulated {days} days -> {} spans (generate {:?}, store+price {:?})", spans.len(), t_gen, t_store);
    println!("compaction (keep {keep_days} d): {} minute rows -> {} hourly + {} daily rows", rep.minute_rows_removed, rep.hourly_rows_written, rep.daily_rows_written);
    println!("database: {} ({:.2} MB)", out.display(), size as f64 / 1_048_576.0);
    println!("total {:.2} kWh, cost {:.2} EUR", sum.total.kwh(), sum.total.cost_minor(2) as f64 / 100.0);
    for (state, t) in &sum.by_state {
        println!("  {:<10} {:>8.2} kWh  {:>7.2} EUR  {:>6.1} h", format!("{state:?}"), t.kwh(), t.cost_minor(2) as f64 / 100.0, t.seconds as f64 / 3600.0);
    }
    for (p, t) in &sum.by_period {
        println!("  period {:<6} {:>6.2} kWh  {:>7.2} EUR", p, t.kwh(), t.cost_minor(2) as f64 / 100.0);
    }
}
