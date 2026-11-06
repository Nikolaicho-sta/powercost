//! powercost-probe: run the M2 pieces against THIS machine.
//!
//!   powercost-probe                  sample all sensors once a second for 10 s and show the state
//!   powercost-probe --samples 30     more samples
//!   powercost-probe --gaps 72        classify the last 72 h as if the tracker had not run,
//!                                    using the real Windows event log

use chrono::{Duration, Utc};
use gap_classifier::{classify_with_source, wevtutil::WevtutilSource, Config, Gap};
use sensors::SensorHub;
use state_detection::{Observation, StateDetector, Thresholds};
use std::time::Duration as StdDuration;

fn arg_value(args: &[String], name: &str) -> Option<u64> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(hours) = arg_value(&args, "--gaps") {
        gaps(hours as i64);
        return;
    }
    let samples = arg_value(&args, "--samples").unwrap_or(10);
    live(samples);
}

fn live(samples: u64) {
    let mut hub = SensorHub::detect();
    println!("== sensors ==");
    for (name, present) in [
        ("cpu_power (Energy Meter)", hub.cpu_power.is_some()),
        ("gpu (NVML)", hub.gpu.is_some()),
        ("cpu_load", hub.cpu_load.is_some()),
        ("input idle", hub.input.is_some()),
        ("power source/battery", hub.power_source.is_some()),
    ] {
        println!("  {:<26} {}", name, if present { "OK" } else { "MISSING" });
    }
    for i in &hub.missing {
        println!("  ! {}: {}", i.sensor, i.message);
    }

    println!("\n== {samples} samples, 1 s apart ==");
    println!("{:>3} {:>8} {:>8} {:>7} {:>9} {:>6} {:>12} {:>7}", "#", "cpu W", "gpu W", "cpu %", "idle s", "src", "batt mW", "state");
    let mut det = StateDetector::new(Thresholds::default());
    for n in 1..=samples {
        std::thread::sleep(StdDuration::from_secs(1));
        let (s, issues) = hub.sample();
        let state = det.update(Observation {
            input_idle_secs: s.input_idle.map(|d| d.as_secs()),
            cpu_percent: s.cpu_load_percent.unwrap_or(0.0),
            gpu_percent: s.gpu.map(|g| g.util_percent),
        });
        let f = |v: Option<f64>| v.map_or("-".to_string(), |x| format!("{x:.1}"));
        let src = s.power_source.map_or("-".to_string(), |p| match (p.ac_online, p.has_battery) {
            (Some(true), true) => if p.charging { "AC+chg".into() } else { "AC".into() },
            (Some(true), false) => "AC".into(),
            (Some(false), _) => "BATT".into(),
            _ => "?".into(),
        });
        println!(
            "{:>3} {:>8} {:>8} {:>7} {:>9} {:>6} {:>12} {:>7}",
            n,
            f(s.cpu_power_w),
            f(s.gpu.map(|g| g.watts)),
            f(s.cpu_load_percent),
            s.input_idle.map_or("-".into(), |d| d.as_secs().to_string()),
            src,
            s.power_source.and_then(|p| p.battery_rate_mw).map_or("-".into(), |r| r.to_string()),
            format!("{:?}", state),
        );
        for i in issues.iter().filter(|i| !hub.missing.contains(i)) {
            println!("    ! {}: {}", i.sensor, i.message);
        }
    }
}

fn gaps(hours: i64) {
    let now = Utc::now();
    let gap = Gap { start: now - Duration::hours(hours), end: now };
    println!("== classifying {} -> {} (as if the tracker had been off) ==", gap.start.format("%F %T"), gap.end.format("%F %T"));
    match classify_with_source(gap, &WevtutilSource, &Config::default()) {
        Err(e) => println!("error: {e}"),
        Ok(r) => {
            let mut totals = std::collections::BTreeMap::<String, i64>::new();
            for s in &r.segments {
                println!(
                    "  {} -> {}  {:>9}  {:<12} {:?}{}",
                    s.start.format("%m-%d %H:%M:%S"),
                    s.end.format("%m-%d %H:%M:%S"),
                    fmt_secs(s.seconds()),
                    format!("{:?}", s.kind),
                    s.confidence,
                    s.sleep
                        .and_then(|d| d.active_permille())
                        .map_or(String::new(), |p| format!("  (sleep was {}.{}% active)", p / 10, p % 10)),
                );
                *totals.entry(format!("{:?}", s.kind)).or_default() += s.seconds();
            }
            println!("\n  totals:");
            for (k, v) in totals {
                println!("    {:<12} {}", k, fmt_secs(v));
            }
        }
    }
}

fn fmt_secs(s: i64) -> String {
    format!("{}h{:02}m{:02}s", s / 3600, (s % 3600) / 60, s % 60)
}
