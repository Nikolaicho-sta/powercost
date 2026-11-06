//! Sensor layer. Every sensor is a trait so the estimator and tests never depend on the OS.
//! Windows implementations live in `windows_impl` and need NO admin rights; anything that is
//! missing is reported as a `SensorIssue` with a human-readable reason (graceful degradation).
//!
//! Sensor values are physical measurements and therefore `f64` watts / percent; they are
//! converted to `Decimal` by the estimator before any money maths.

pub mod mock;
#[cfg(windows)]
pub mod windows_impl;

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SensorError {
    /// This machine does not provide the sensor (reason is shown to the user).
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// The sensor exists but this read failed.
    #[error("read failed: {0}")]
    Failed(String),
}

pub trait CpuPowerSensor: Send {
    /// CPU package power in watts.
    fn read_watts(&mut self) -> Result<f64, SensorError>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuReading {
    pub watts: f64,
    pub util_percent: f64,
}

pub trait GpuPowerSensor: Send {
    fn read(&mut self) -> Result<GpuReading, SensorError>;
}

pub trait CpuLoadSensor: Send {
    /// Load since the previous call, 0..=100. The first call may return 0.
    fn read_percent(&mut self) -> Result<f64, SensorError>;
}

pub trait InputActivitySensor: Send {
    fn idle_for(&mut self) -> Result<Duration, SensorError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSourceReading {
    pub has_battery: bool,
    /// `None` when Windows does not know.
    pub ac_online: Option<bool>,
    pub charging: bool,
    pub battery_percent: Option<u8>,
    /// Instantaneous battery rate in mW: negative = discharging, positive = charging.
    pub battery_rate_mw: Option<i32>,
}

pub trait PowerSourceSensor: Send {
    fn read(&mut self) -> Result<PowerSourceReading, SensorError>;
}

/// A sensor that could not be used, with the reason to show in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorIssue {
    pub sensor: &'static str,
    pub message: String,
}

/// One combined reading.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub cpu_power_w: Option<f64>,
    pub gpu: Option<GpuReading>,
    pub cpu_load_percent: Option<f64>,
    pub input_idle: Option<Duration>,
    pub power_source: Option<PowerSourceReading>,
}

#[derive(Default)]
pub struct SensorHub {
    pub cpu_power: Option<Box<dyn CpuPowerSensor>>,
    pub gpu: Option<Box<dyn GpuPowerSensor>>,
    pub cpu_load: Option<Box<dyn CpuLoadSensor>>,
    pub input: Option<Box<dyn InputActivitySensor>>,
    pub power_source: Option<Box<dyn PowerSourceSensor>>,
    /// Reasons recorded at detection time for sensors that are missing entirely.
    pub missing: Vec<SensorIssue>,
}

impl SensorHub {
    /// Read every available sensor. Read failures become issues; they never abort the sample.
    pub fn sample(&mut self) -> (Snapshot, Vec<SensorIssue>) {
        let mut snap = Snapshot::default();
        let mut issues = self.missing.clone();

        if let Some(s) = self.cpu_power.as_mut() {
            match s.read_watts() {
                Ok(w) => snap.cpu_power_w = Some(w),
                Err(e) => issues.push(SensorIssue { sensor: "cpu_power", message: e.to_string() }),
            }
        }
        if let Some(s) = self.gpu.as_mut() {
            match s.read() {
                Ok(r) => snap.gpu = Some(r),
                Err(e) => issues.push(SensorIssue { sensor: "gpu", message: e.to_string() }),
            }
        }
        if let Some(s) = self.cpu_load.as_mut() {
            match s.read_percent() {
                Ok(p) => snap.cpu_load_percent = Some(p),
                Err(e) => issues.push(SensorIssue { sensor: "cpu_load", message: e.to_string() }),
            }
        }
        if let Some(s) = self.input.as_mut() {
            match s.idle_for() {
                Ok(d) => snap.input_idle = Some(d),
                Err(e) => issues.push(SensorIssue { sensor: "input", message: e.to_string() }),
            }
        }
        if let Some(s) = self.power_source.as_mut() {
            match s.read() {
                Ok(r) => snap.power_source = Some(r),
                Err(e) => issues.push(SensorIssue { sensor: "power_source", message: e.to_string() }),
            }
        }
        (snap, issues)
    }
}

