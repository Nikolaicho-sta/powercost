//! Plugin interface for EXTERNAL wall-power sources, for example a smart plug that reports the
//! real watts drawn by this PC (Shelly, Tasmota, Home Assistant, ...).
//!
//! Status: INTERFACE ONLY. No implementation ships yet. The estimator does not consume these
//! readings yet either; the planned integration is a new top layer, "wall measurement", above the
//! sensor layer (see docs/DECISIONS.md D34). Keeping the contract here, with tests, means a
//! source can be added later without touching the tracker, storage or tariff code.
//!
//! Contract for implementors:
//! * `read` must return quickly (the tracker calls it on its sampling tick) and must never block
//!   indefinitely; do network I/O with a short timeout.
//! * The reading must be the draw of THIS PC only (a plug on the PC's own cable), in watts at the
//!   wall, including the PSU/adapter losses.
//! * Sources are opt-in and local-network only by design; any internet access must be documented
//!   in docs/PRIVACY.md before it exists.

use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SourceError {
    /// The source is not reachable or not configured right now (shown to the user, not fatal).
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// The source answered with something unusable.
    #[error("bad reading: {0}")]
    Invalid(String),
}

/// What kind of device a source talks to (for the UI; implementors pick one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceKind {
    Shelly,
    Tasmota,
    HomeAssistant,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallReading {
    /// Watts drawn from the wall by this PC.
    pub watts: f64,
    /// When the device measured it (not when we asked).
    pub at: DateTime<Utc>,
}

pub trait WallPowerSource: Send {
    /// Stable identifier, unique within a registry, e.g. `shelly:192.168.1.20`.
    fn id(&self) -> &str;
    fn kind(&self) -> SourceKind;
    /// Short human description for the settings screen.
    fn describe(&self) -> String;
    fn read(&mut self) -> Result<WallReading, SourceError>;
    /// Readings older than this are ignored.
    fn max_age(&self) -> Duration {
        Duration::seconds(30)
    }
}

/// Accept only physically plausible readings (a PC does not draw NaN, negative or > 5 kW).
pub fn validate_reading(r: &WallReading) -> Result<(), SourceError> {
    if !r.watts.is_finite() || r.watts < 0.0 || r.watts > 5000.0 {
        return Err(SourceError::Invalid(format!("{} W is not a plausible PC draw", r.watts)));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceIssue {
    pub source_id: String,
    pub message: String,
}

#[derive(Default)]
pub struct SourceRegistry {
    sources: Vec<Box<dyn WallPowerSource>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ids must be unique so settings and issues can refer to a source unambiguously.
    pub fn register(&mut self, source: Box<dyn WallPowerSource>) -> Result<(), String> {
        if self.sources.iter().any(|s| s.id() == source.id()) {
            return Err(format!("a source with id '{}' is already registered", source.id()));
        }
        self.sources.push(source);
        Ok(())
    }

    pub fn ids(&self) -> Vec<String> {
        self.sources.iter().map(|s| s.id().to_string()).collect()
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// The first registered source that answers with a plausible, fresh reading, plus the problems
    /// of the sources tried before it. With no healthy source the result is `None`.
    pub fn latest(&mut self, now: DateTime<Utc>) -> (Option<(String, WallReading)>, Vec<SourceIssue>) {
        let mut issues = Vec::new();
        for s in self.sources.iter_mut() {
            let id = s.id().to_string();
            match s.read().and_then(|r| validate_reading(&r).map(|_| r)) {
                Ok(r) if now - r.at <= s.max_age() => return (Some((id, r)), issues),
                Ok(r) => issues.push(SourceIssue { source_id: id, message: format!("reading is {} s old", (now - r.at).num_seconds()) }),
                Err(e) => issues.push(SourceIssue { source_id: id, message: e.to_string() }),
            }
        }
        (None, issues)
    }
}