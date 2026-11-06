//! Shared vocabulary types for PowerCost crates.

use serde::{Deserialize, Serialize};

/// Power state of the machine during an interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerState {
    Active,
    Idle,
    Sleep,
    Hibernate,
    Off,
}

impl PowerState {
    pub const ALL: [PowerState; 5] =
        [PowerState::Active, PowerState::Idle, PowerState::Sleep, PowerState::Hibernate, PowerState::Off];
}

/// How a wattage figure was obtained (shown next to every number in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Measured by sensors (still partial: components, not the wall).
    MeasuredSensors,
    /// User-calibrated with a wall wattmeter.
    CalibratedModel,
    /// Derived from detected hardware and defaults.
    DefaultModel,
}

/// Fixed-point scale for stored energy and money: 1e-9 of a kWh / of a currency unit.
pub const NANO: i64 = 1_000_000_000;
