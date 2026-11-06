//! Raw Windows event representation and parsing of `wevtutil qe ... /f:xml` output.

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

/// One System-log event, reduced to what the classifier needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerEvent {
    pub time: DateTime<Utc>,
    /// Provider name as logged, e.g. "Microsoft-Windows-Kernel-Power".
    pub provider: String,
    pub id: u32,
    /// Named `<Data Name="...">` fields.
    pub data: BTreeMap<String, String>,
}

impl PowerEvent {
    pub fn new(time: DateTime<Utc>, provider: &str, id: u32) -> Self {
        Self { time, provider: provider.to_string(), id, data: BTreeMap::new() }
    }

    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.data.insert(key.to_string(), value.to_string());
        self
    }

    /// Provider name without the "Microsoft-Windows-" prefix.
    pub fn short_provider(&self) -> &str {
        self.provider.strip_prefix("Microsoft-Windows-").unwrap_or(&self.provider)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(String::as_str)
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key)?.trim().parse().ok()
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("XML error: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("event without a valid SystemTime")]
    BadTime,
}

/// Parse the output of `wevtutil qe System /f:xml`. wevtutil prints several `<Event>`
/// elements with no root, so the text is wrapped in `<Events>` if needed.
pub fn parse_events_xml(xml: &str) -> Result<Vec<PowerEvent>, ParseError> {
    let trimmed = xml.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let wrapped;
    let text = if trimmed.starts_with("<Events") {
        trimmed
    } else {
        wrapped = format!("<Events>{trimmed}</Events>");
        &wrapped
    };
    let doc = roxmltree::Document::parse(text)?;
    let mut out = Vec::new();
    for ev in doc.descendants().filter(|n| n.has_tag_name("Event")) {
        let Some(system) = ev.children().find(|n| n.has_tag_name("System")) else { continue };
        let provider = system
            .children()
            .find(|n| n.has_tag_name("Provider"))
            .and_then(|n| n.attribute("Name"))
            .unwrap_or("")
            .to_string();
        let id = system
            .children()
            .find(|n| n.has_tag_name("EventID"))
            .and_then(|n| n.text())
            .and_then(|t| t.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let time = system
            .children()
            .find(|n| n.has_tag_name("TimeCreated"))
            .and_then(|n| n.attribute("SystemTime"))
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
            .ok_or(ParseError::BadTime)?;
        let mut data = BTreeMap::new();
        if let Some(ed) = ev.children().find(|n| n.has_tag_name("EventData")) {
            for d in ed.children().filter(|n| n.has_tag_name("Data")) {
                if let Some(name) = d.attribute("Name") {
                    data.insert(name.to_string(), d.text().unwrap_or("").to_string());
                }
            }
        }
        out.push(PowerEvent { time, provider, id, data });
    }
    out.sort_by_key(|e| e.time);
    Ok(out)
}
