//! ACTIVE / IDLE detection for a powered-on PC.
//!
//! ACTIVE = recent user input, or meaningful CPU/GPU load. Load uses hysteresis (enter at a
//! higher level than the level needed to stay) and a debounce count so a brief dip or spike
//! does not flap the state. Sleep/off/hibernate are not detected here: the tracker is not
//! running then (see `gap-classifier`).

use pc_core::PowerState;
use serde::{Deserialize, Serialize};

/// User-configurable thresholds (Settings > Thresholds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thresholds {
    /// No input for this many seconds => input no longer counts as activity.
    pub input_idle_secs: u64,
    /// CPU load (%) at or above which the PC counts as active.
    pub cpu_enter_percent: u32,
    /// Once active because of load, stay active until load drops below this (%).
    pub cpu_exit_percent: u32,
    pub gpu_enter_percent: u32,
    pub gpu_exit_percent: u32,
    /// Consecutive "below exit" observations needed before leaving load-driven activity.
    pub exit_debounce_ticks: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            input_idle_secs: 120,
            cpu_enter_percent: 20,
            cpu_exit_percent: 10,
            gpu_enter_percent: 25,
            gpu_exit_percent: 10,
            exit_debounce_ticks: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// Seconds since the last keyboard/mouse input; `None` if unknown.
    pub input_idle_secs: Option<u64>,
    /// 0..=100 (values above 100, e.g. "processor utility", are clamped).
    pub cpu_percent: f64,
    pub gpu_percent: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct StateDetector {
    cfg: Thresholds,
    load_active: bool,
    below_ticks: u32,
}

impl StateDetector {
    pub fn new(cfg: Thresholds) -> Self {
        Self { cfg, load_active: false, below_ticks: 0 }
    }

    pub fn set_thresholds(&mut self, cfg: Thresholds) {
        self.cfg = cfg;
    }

    /// Feed one observation (called once per sampling tick) and get the current state.
    pub fn update(&mut self, o: Observation) -> PowerState {
        let c = &self.cfg;
        let input_active = o.input_idle_secs.is_some_and(|s| s < c.input_idle_secs);

        let cpu = o.cpu_percent.clamp(0.0, 100.0);
        let gpu = o.gpu_percent.unwrap_or(0.0).clamp(0.0, 100.0);
        let enter = cpu >= c.cpu_enter_percent as f64 || gpu >= c.gpu_enter_percent as f64;
        let stay = cpu >= c.cpu_exit_percent as f64 || gpu >= c.gpu_exit_percent as f64;

        if enter {
            self.load_active = true;
            self.below_ticks = 0;
        } else if self.load_active {
            if stay {
                self.below_ticks = 0;
            } else {
                self.below_ticks += 1;
                if self.below_ticks >= c.exit_debounce_ticks {
                    self.load_active = false;
                    self.below_ticks = 0;
                }
            }
        }

        if input_active || self.load_active {
            PowerState::Active
        } else {
            PowerState::Idle
        }
    }
}
