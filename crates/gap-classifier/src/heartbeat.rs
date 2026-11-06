//! Heartbeat bookkeeping and gap detection (storage is added in M3; this is the pure logic).

use crate::Gap;
use chrono::{DateTime, Duration, Utc};
use pc_core::PowerState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerSource {
    Ac,
    Battery,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub time: DateTime<Utc>,
    /// Random id per tracker process; a change means the tracker restarted.
    pub session_id: String,
    pub state: PowerState,
    pub source: PowerSource,
}

#[derive(Debug, Clone, Copy)]
pub struct HeartbeatConfig {
    pub interval: Duration,
    /// A gap is declared when the silence exceeds `interval * tolerance_factor`.
    pub tolerance_factor: i32,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self { interval: Duration::seconds(30), tolerance_factor: 3 }
    }
}

/// Returns the gap between the last stored heartbeat and `now`, if it is longer than tolerated.
/// Also a gap if the session id changed (tracker restarted), even when short, so restarts are
/// classified against the event log instead of silently ignored.
pub fn detect_gap(last: Option<&Heartbeat>, now: DateTime<Utc>, session_id: &str, cfg: &HeartbeatConfig) -> Option<Gap> {
    let last = last?;
    if now <= last.time {
        return None; // clock went backwards or duplicate; nothing to cost
    }
    let silent = now - last.time;
    let restarted = last.session_id != session_id;
    if silent > cfg.interval * cfg.tolerance_factor || (restarted && silent > cfg.interval) {
        Some(Gap { start: last.time, end: now })
    } else {
        None
    }
}
