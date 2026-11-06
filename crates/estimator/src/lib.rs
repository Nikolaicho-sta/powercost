//! Power estimator: turns sensor readings (or just a power state) into estimated WALL watts,
//! and says how it got there.
//!
//! Layers (see docs/DECISIONS.md D13):
//!   Sensors    - CPU (+GPU) power measured by the OS/driver, the rest of the system modelled,
//!                divided by PSU/adapter efficiency. Confidence: measured sensors (partial).
//!   Calibrated - user-measured wall watts at idle / full load (+ optional points), linearly
//!                interpolated by utilisation. Confidence: calibrated model.
//!   Default    - from detected hardware (TDPs, drives) plus a baseline. Widest uncertainty.
//! Chain order is configurable: by default a wall-meter calibration wins over partial
//! component sensors (`prefer_calibrated`), otherwise Sensors -> Calibrated -> Default.
//!
//! Every estimate carries its layer, an uncertainty range and a one-line method description
//! for the UI tooltip. Watts are `Decimal` (2 dp); sensors give f64 which is converted here.

use pc_core::{Confidence, PowerState};
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EstimatorError {
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Sensors,
    Calibrated,
    Default,
}

impl Layer {
    pub fn confidence(self) -> Confidence {
        match self {
            Layer::Sensors => Confidence::MeasuredSensors,
            Layer::Calibrated => Confidence::CalibratedModel,
            Layer::Default => Confidence::DefaultModel,
        }
    }
}

/// Detected / entered hardware used by the default model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hardware {
    pub cpu_tdp_w: Decimal,
    pub cpu_idle_w: Decimal,
    /// `None` = no discrete GPU.
    pub gpu_tdp_w: Option<Decimal>,
    pub gpu_idle_w: Decimal,
    pub drives: u32,
}

impl Default for Hardware {
    fn default() -> Self {
        Self { cpu_tdp_w: dec(65), cpu_idle_w: dec(5), gpu_tdp_w: None, gpu_idle_w: dec(0), drives: 1 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationPoint {
    /// Utilisation in per-mille (0 = idle, 1000 = full load).
    pub util_permille: u32,
    pub watts: Decimal,
}

/// Wall-meter measurements. Needs at least idle (0) and full (1000).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    pub points: Vec<CalibrationPoint>,
}

impl Calibration {
    pub fn new(idle_w: Decimal, full_w: Decimal) -> Result<Self, EstimatorError> {
        Self::from_points(vec![
            CalibrationPoint { util_permille: 0, watts: idle_w },
            CalibrationPoint { util_permille: 1000, watts: full_w },
        ])
    }

    pub fn from_points(mut points: Vec<CalibrationPoint>) -> Result<Self, EstimatorError> {
        points.sort_by_key(|p| p.util_permille);
        let c = Self { points };
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> Result<(), EstimatorError> {
        let bad = |m: &str| Err(EstimatorError::Invalid(m.to_string()));
        if self.points.first().map(|p| p.util_permille) != Some(0) {
            return bad("calibration needs an idle (0%) point");
        }
        if self.points.last().map(|p| p.util_permille) != Some(1000) {
            return bad("calibration needs a full-load (100%) point");
        }
        if self.points.windows(2).any(|w| w[0].util_permille == w[1].util_permille) {
            return bad("duplicate calibration utilisation");
        }
        if self.points.iter().any(|p| p.watts <= Decimal::ZERO || p.watts > dec(5000)) {
            return bad("calibration watts must be between 0 and 5000");
        }
        if self.points[0].watts >= self.points.last().unwrap().watts {
            return bad("full-load watts must be higher than idle watts");
        }
        Ok(())
    }

    /// Piecewise-linear interpolation; clamps outside 0..=1000.
    pub fn watts_at(&self, util_permille: u32) -> Decimal {
        let u = util_permille.min(1000);
        for w in self.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            if u <= b.util_permille {
                let span = Decimal::from(b.util_permille - a.util_permille);
                let frac = Decimal::from(u - a.util_permille) / span;
                return a.watts + (b.watts - a.watts) * frac;
            }
        }
        self.points.last().unwrap().watts
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimatorConfig {
    /// PSU (desktop) or AC adapter (laptop) efficiency, 0.5..=1.0.
    pub psu_efficiency: Decimal,
    /// Rest of system (board, RAM, fans, panel...) excluding CPU/GPU/drives, DC watts.
    pub baseline_w: Decimal,
    pub per_drive_w: Decimal,
    /// Optional separate monitor draw, counted only while ACTIVE. Off by default.
    pub include_monitor: bool,
    pub monitor_w: Decimal,
    pub sleep_w: Decimal,
    pub hibernate_w: Decimal,
    /// Draw of a shut-down PC (PSU/board standby).
    pub off_standby_w: Decimal,
    /// "I switch it off at the wall": shut-down draw is 0 W.
    pub unplugged_when_off: bool,
    pub prefer_calibrated: bool,
    pub hardware: Hardware,
    pub calibration: Option<Calibration>,
}

impl Default for EstimatorConfig {
    fn default() -> Self {
        Self {
            psu_efficiency: Decimal::new(85, 2),
            baseline_w: dec(20),
            per_drive_w: Decimal::new(25, 1),
            include_monitor: false,
            monitor_w: dec(25),
            sleep_w: dec(1),
            hibernate_w: dec(0),
            off_standby_w: dec(1),
            unplugged_when_off: false,
            prefer_calibrated: true,
            hardware: Hardware::default(),
            calibration: None,
        }
    }
}

impl EstimatorConfig {
    pub fn validate(&self) -> Result<(), EstimatorError> {
        let bad = |m: &str| Err(EstimatorError::Invalid(m.to_string()));
        if self.psu_efficiency < Decimal::new(5, 1) || self.psu_efficiency > Decimal::ONE {
            return bad("PSU efficiency must be between 0.50 and 1.00");
        }
        for v in [self.baseline_w, self.per_drive_w, self.monitor_w, self.sleep_w, self.hibernate_w, self.off_standby_w] {
            if v < Decimal::ZERO {
                return bad("watt values must not be negative");
            }
        }
        if let Some(c) = &self.calibration {
            c.validate()?;
        }
        Ok(())
    }
}

/// Everything the estimator needs for one span. Stored with the span so history can be
/// re-estimated after a calibration change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inputs {
    pub state: PowerState,
    pub cpu_power_w: Option<f64>,
    pub gpu_power_w: Option<f64>,
    pub cpu_load_pct: f64,
    pub gpu_util_pct: f64,
    /// Battery charge power in watts while on AC (DC side), if known.
    pub charge_w: Option<f64>,
    /// For Sleep: share of the sleep spent actively running (Modern Standby), per-mille.
    pub sleep_active_permille: Option<u32>,
}

impl Inputs {
    pub fn new(state: PowerState) -> Self {
        Self { state, cpu_power_w: None, gpu_power_w: None, cpu_load_pct: 0.0, gpu_util_pct: 0.0, charge_w: None, sleep_active_permille: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub watts: Decimal,
    pub low: Decimal,
    pub high: Decimal,
    pub layer: Layer,
    /// One-line explanation for the UI info tooltip.
    pub method: String,
}

impl Estimate {
    pub fn confidence(&self) -> Confidence {
        self.layer.confidence()
    }
}

fn dec(n: i64) -> Decimal {
    Decimal::from(n)
}

fn from_f64(x: f64) -> Decimal {
    Decimal::from_f64(x).unwrap_or_default().round_dp(2)
}

fn uncertainty_permille(l: Layer) -> i64 {
    match l {
        Layer::Sensors => 150,
        Layer::Calibrated => 50,
        Layer::Default => 300,
    }
}

fn estimate_of(watts: Decimal, layer: Layer, extra_permille: i64, method: String) -> Estimate {
    let u = Decimal::new(uncertainty_permille(layer) + extra_permille, 3);
    let watts = watts.round_dp(2);
    Estimate {
        watts,
        low: (watts * (Decimal::ONE - u)).max(Decimal::ZERO).round_dp(2),
        high: (watts * (Decimal::ONE + u)).round_dp(2),
        layer,
        method,
    }
}

pub struct Estimator {
    cfg: EstimatorConfig,
}

impl Estimator {
    pub fn new(cfg: EstimatorConfig) -> Result<Self, EstimatorError> {
        cfg.validate()?;
        Ok(Self { cfg })
    }

    pub fn config(&self) -> &EstimatorConfig {
        &self.cfg
    }

    fn rest_of_system(&self) -> Decimal {
        self.cfg.baseline_w + self.cfg.per_drive_w * Decimal::from(self.cfg.hardware.drives)
    }

    fn model_cpu(&self, load_pct: f64) -> Decimal {
        let h = &self.cfg.hardware;
        h.cpu_idle_w + (h.cpu_tdp_w - h.cpu_idle_w) * from_f64(load_pct.clamp(0.0, 100.0)) / dec(100)
    }

    fn model_gpu(&self, util_pct: f64) -> Decimal {
        let h = &self.cfg.hardware;
        match h.gpu_tdp_w {
            Some(tdp) => h.gpu_idle_w + (tdp - h.gpu_idle_w) * from_f64(util_pct.clamp(0.0, 100.0)) / dec(100),
            None => Decimal::ZERO,
        }
    }

    fn layer_order(&self) -> [Layer; 3] {
        if self.cfg.prefer_calibrated {
            [Layer::Calibrated, Layer::Sensors, Layer::Default]
        } else {
            [Layer::Sensors, Layer::Calibrated, Layer::Default]
        }
    }

    /// Wall watts of a running PC (ACTIVE or IDLE), through the fallback chain.
    fn running(&self, i: &Inputs, with_monitor: bool) -> Estimate {
        let eff = self.cfg.psu_efficiency;
        let charge = i.charge_w.map(from_f64).unwrap_or_default().max(Decimal::ZERO);
        let monitor = if with_monitor && self.cfg.include_monitor { self.cfg.monitor_w } else { Decimal::ZERO };
        let util = (i.cpu_load_pct.max(i.gpu_util_pct).clamp(0.0, 100.0) * 10.0).round() as u32;

        for layer in self.layer_order() {
            match layer {
                Layer::Sensors => {
                    let Some(cpu) = i.cpu_power_w else { continue };
                    let cpu = from_f64(cpu);
                    let (gpu, gpu_note) = match (i.gpu_power_w, self.cfg.hardware.gpu_tdp_w) {
                        (Some(g), _) => (from_f64(g), "GPU measured"),
                        (None, Some(_)) => (self.model_gpu(i.gpu_util_pct), "GPU modelled"),
                        (None, None) => (Decimal::ZERO, "no GPU"),
                    };
                    let dc = cpu + gpu + self.rest_of_system() + charge;
                    return estimate_of(
                        dc / eff + monitor,
                        Layer::Sensors,
                        0,
                        format!(
                            "CPU {cpu} W measured, {gpu_note} ({gpu} W), rest of system modelled ({} W){}, divided by PSU efficiency {}%",
                            self.rest_of_system(),
                            if charge > Decimal::ZERO { format!(", battery charging {charge} W") } else { String::new() },
                            (eff * dec(100)).round_dp(0)
                        ),
                    );
                }
                Layer::Calibrated => {
                    let Some(cal) = &self.cfg.calibration else { continue };
                    let w = cal.watts_at(util) + charge / eff + monitor;
                    return estimate_of(
                        w,
                        Layer::Calibrated,
                        0,
                        format!(
                            "interpolated from your wall-meter calibration ({} W idle, {} W full) at {}% utilisation",
                            cal.points[0].watts,
                            cal.points.last().unwrap().watts,
                            util / 10
                        ),
                    );
                }
                Layer::Default => {
                    let cpu = self.model_cpu(i.cpu_load_pct);
                    let gpu = self.model_gpu(i.gpu_util_pct);
                    let dc = cpu + gpu + self.rest_of_system() + charge;
                    return estimate_of(
                        dc / eff + monitor,
                        Layer::Default,
                        0,
                        format!(
                            "default model: CPU {cpu} W + GPU {gpu} W (from TDP and load) + rest of system {} W, divided by PSU efficiency {}%",
                            self.rest_of_system(),
                            (eff * dec(100)).round_dp(0)
                        ),
                    );
                }
            }
        }
        unreachable!("Default layer always returns")
    }

    /// Idle PC with the screen off and no sensors (what a "sleep" that is really running draws).
    pub fn idle_running_estimate(&self) -> Estimate {
        self.running(&Inputs::new(PowerState::Idle), false)
    }

    pub fn estimate(&self, i: &Inputs) -> Estimate {
        match i.state {
            PowerState::Active => self.running(i, true),
            PowerState::Idle => self.running(i, false),
            PowerState::Sleep => {
                let sleep_w = self.cfg.sleep_w;
                match i.sleep_active_permille {
                    Some(p) => {
                        let p = p.min(1000);
                        let idle = self.idle_running_estimate();
                        let share = Decimal::new(p as i64, 3);
                        let w = idle.watts * share + sleep_w * (Decimal::ONE - share);
                        estimate_of(
                            w,
                            idle.layer,
                            100,
                            format!(
                                "Modern Standby: {}.{}% of the time actively running at idle power ({} W), the rest at sleep power ({} W)",
                                p / 10,
                                p % 10,
                                idle.watts,
                                sleep_w
                            ),
                        )
                    }
                    None => estimate_of(sleep_w, Layer::Default, 0, format!("configured sleep power ({sleep_w} W)")),
                }
            }
            PowerState::Hibernate => {
                estimate_of(self.cfg.hibernate_w, Layer::Default, 0, format!("configured hibernate power ({} W)", self.cfg.hibernate_w))
            }
            PowerState::Off => {
                if self.cfg.unplugged_when_off {
                    estimate_of(Decimal::ZERO, Layer::Default, 0, "switched off at the wall: 0 W".into())
                } else {
                    estimate_of(
                        self.cfg.off_standby_w,
                        Layer::Default,
                        0,
                        format!("PSU/board standby draw while shut down ({} W)", self.cfg.off_standby_w),
                    )
                }
            }
        }
    }
}
