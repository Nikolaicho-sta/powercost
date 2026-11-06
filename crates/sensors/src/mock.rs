//! Scriptable sensors for tests and the simulator.

use crate::*;
use std::time::Duration;

#[derive(Default)]
pub struct MockCpuPower(pub Option<f64>);
impl CpuPowerSensor for MockCpuPower {
    fn read_watts(&mut self) -> Result<f64, SensorError> {
        self.0.ok_or_else(|| SensorError::Unavailable("mock: no CPU power".into()))
    }
}

#[derive(Default)]
pub struct MockGpu(pub Option<GpuReading>);
impl GpuPowerSensor for MockGpu {
    fn read(&mut self) -> Result<GpuReading, SensorError> {
        self.0.ok_or_else(|| SensorError::Unavailable("mock: no GPU".into()))
    }
}

#[derive(Default)]
pub struct MockCpuLoad(pub f64);
impl CpuLoadSensor for MockCpuLoad {
    fn read_percent(&mut self) -> Result<f64, SensorError> {
        Ok(self.0)
    }
}

pub struct MockInput(pub Duration);
impl InputActivitySensor for MockInput {
    fn idle_for(&mut self) -> Result<Duration, SensorError> {
        Ok(self.0)
    }
}

pub struct MockPowerSource(pub PowerSourceReading);
impl PowerSourceSensor for MockPowerSource {
    fn read(&mut self) -> Result<PowerSourceReading, SensorError> {
        Ok(self.0)
    }
}
