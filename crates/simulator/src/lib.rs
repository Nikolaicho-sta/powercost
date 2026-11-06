//! Simulator: deterministic fake usage for testing the UI and the storage/cost pipeline
//! without waiting weeks. A "typical" laptop user: office hours, lunch, evening gaming on some
//! days, nights spent shut down or in Modern Standby (mostly active, like the reference laptop).
//! Same seed => same data.

use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use estimator::{Estimator, Inputs};
use pc_core::PowerState;
use storage::{Span, SpanSource};

#[derive(Debug, Clone, Copy)]
pub struct SimConfig {
    pub seed: u64,
    /// Report CPU/GPU power sensor values (exercises the Sensors layer).
    pub with_sensors: bool,
    /// Laptop: an hour on battery each weekday afternoon (not billed).
    pub laptop: bool,
    /// Per-mille of nights the PC is shut down (otherwise it sleeps).
    pub shutdown_night_permille: u64,
    /// Per-mille of days with an evening gaming session.
    pub gaming_permille: u64,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self { seed: 42, with_sensors: true, laptop: true, shutdown_night_permille: 400, gaming_permille: 450 }
    }
}

/// splitmix64: tiny, deterministic, good enough for fake data.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Uniform in [0, n).
fn roll(seed: u64, a: u64, b: u64, n: u64) -> u64 {
    mix(mix(seed ^ a).wrapping_add(b)) % n
}

fn day_key(d: NaiveDate) -> u64 {
    d.num_days_from_ce() as u64
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Off,
    Sleep,
    Idle,
    Office,
    Gaming,
    LightUse,
}

fn mode_at(cfg: &SimConfig, local_date: NaiveDate, hour: f64) -> Mode {
    let weekend = matches!(local_date.weekday(), Weekday::Sat | Weekday::Sun);
    let k = day_key(local_date);
    let wake = if weekend { 9.5 } else { 7.5 } + roll(cfg.seed, k, 1, 60) as f64 / 60.0;
    let gaming = roll(cfg.seed, k, 2, 1000) < cfg.gaming_permille;
    // The night that STARTS on the previous evening decides the early-morning state.
    let prev_night_off = roll(cfg.seed, k.wrapping_sub(1), 3, 1000) < cfg.shutdown_night_permille;
    let tonight_off = roll(cfg.seed, k, 3, 1000) < cfg.shutdown_night_permille;

    if hour < wake {
        return if prev_night_off { Mode::Off } else { Mode::Sleep };
    }
    if hour >= 23.5 {
        return if tonight_off { Mode::Off } else { Mode::Sleep };
    }
    if weekend {
        return match hour {
            h if h < 11.0 => Mode::Idle,
            h if h < 18.0 => if gaming { Mode::Gaming } else { Mode::LightUse },
            h if h < 20.0 => Mode::Idle,
            _ => if gaming { Mode::Gaming } else { Mode::LightUse },
        };
    }
    match hour {
        h if h < 8.5 => Mode::Idle,
        h if h < 12.5 => Mode::Office,
        h if h < 13.5 => Mode::Idle,
        h if h < 17.5 => Mode::Office,
        h if h < 19.0 => Mode::Idle,
        _ => if gaming { Mode::Gaming } else { Mode::LightUse },
    }
}

fn inputs_for(cfg: &SimConfig, mode: Mode, minute_key: u64) -> Inputs {
    let jitter = |salt: u64, lo: u64, hi: u64| (lo + roll(cfg.seed, minute_key, salt, hi - lo + 1)) as f64;
    let (state, cpu, gpu) = match mode {
        Mode::Off => return Inputs::new(PowerState::Off),
        Mode::Sleep => {
            // Modern Standby: mostly active (see D9), small variation per night.
            return Inputs {
                sleep_active_permille: Some(955 + (roll(cfg.seed, minute_key / 600, 9, 30)) as u32),
                ..Inputs::new(PowerState::Sleep)
            };
        }
        Mode::Idle => (PowerState::Idle, jitter(1, 1, 6), jitter(2, 0, 3)),
        Mode::LightUse => (PowerState::Active, jitter(1, 8, 25), jitter(2, 2, 12)),
        Mode::Office => (PowerState::Active, jitter(1, 15, 60), jitter(2, 3, 18)),
        Mode::Gaming => (PowerState::Active, jitter(1, 35, 80), jitter(2, 60, 98)),
    };
    let mut i = Inputs::new(state);
    i.cpu_load_pct = cpu;
    i.gpu_util_pct = gpu;
    if cfg.with_sensors {
        i.cpu_power_w = Some(5.0 + 40.0 * cpu / 100.0 + jitter(3, 0, 20) / 10.0);
        i.gpu_power_w = Some(5.0 + 55.0 * gpu / 100.0 + jitter(4, 0, 20) / 10.0);
    }
    i
}

/// Generate minute-resolution spans for `[from, to)` (whole minutes, UTC), merging runs of
/// identical minutes (nights) into long spans like a real gap would.
pub fn generate(cfg: &SimConfig, est: &Estimator, from: DateTime<Utc>, to: DateTime<Utc>, tz: Tz) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut cur = from;
    while cur < to {
        let next = cur + Duration::minutes(1);
        let local = cur.with_timezone(&tz);
        let hour = local.hour() as f64 + local.minute() as f64 / 60.0;
        let mode = mode_at(cfg, local.date_naive(), hour);
        let key = cur.timestamp() as u64 / 60;
        let inputs = inputs_for(cfg, mode, key);
        let weekday = !matches!(local.weekday(), Weekday::Sat | Weekday::Sun);
        let on_battery = cfg.laptop && weekday && (14.0..15.0).contains(&hour) && mode == Mode::Office;
        let e = est.estimate(&inputs);
        let billable = !on_battery;

        let merge = out.last().is_some_and(|l| {
            l.end == cur && l.inputs == inputs && l.billable == billable && matches!(l.inputs.state, PowerState::Off | PowerState::Sleep)
        });
        if merge {
            out.last_mut().unwrap().end = next;
        } else {
            out.push(Span { start: cur, end: next, inputs, watts: e.watts, layer: e.layer, billable, source: SpanSource::Simulated });
        }
        cur = next;
    }
    out
}
