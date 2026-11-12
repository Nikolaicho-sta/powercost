//! Reads the Windows System log through the built-in `wevtutil.exe` (no admin needed,
//! no unsafe FFI; runs at startup, after a sleep/stall gap and every 30 minutes to copy recent
//! events, always with its console window hidden). See docs/DECISIONS.md D8.

use crate::{event::parse_events_xml, EventSource, PowerEvent};
use chrono::{DateTime, SecondsFormat, Utc};
use std::process::Command;

pub struct WevtutilSource;

/// (provider, event ids) the classifier understands.
const WANTED: &[(&str, &[u32])] = &[
    ("Microsoft-Windows-Kernel-Power", &[41, 42, 107, 109, 506, 507]),
    ("Microsoft-Windows-Power-Troubleshooter", &[1]),
    ("Microsoft-Windows-Kernel-General", &[12, 13]),
    ("Microsoft-Windows-Kernel-Boot", &[20, 27]),
    ("EventLog", &[6005, 6006, 6008]),
    ("User32", &[1074]),
];

pub fn build_query(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    let providers: Vec<String> = WANTED
        .iter()
        .map(|(p, ids)| {
            let ids: Vec<String> = ids.iter().map(|i| format!("EventID={i}")).collect();
            format!("(Provider[@Name='{p}'] and ({}))", ids.join(" or "))
        })
        .collect();
    format!(
        "*[System[TimeCreated[@SystemTime>='{}' and @SystemTime<='{}'] and ({})]]",
        from.to_rfc3339_opts(SecondsFormat::Millis, true),
        to.to_rfc3339_opts(SecondsFormat::Millis, true),
        providers.join(" or ")
    )
}

impl EventSource for WevtutilSource {
    fn events(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<PowerEvent>, String> {
        let mut cmd = Command::new("wevtutil");
        cmd.args(["qe", "System", &format!("/q:{}", build_query(from, to)), "/f:xml", "/rd:false"]);
        // The tracker runs this every 30 minutes from a window-less GUI process; without this flag
        // Windows opens a console window for a split second each time (it flashed over full-screen games).
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd
            .output()
            .map_err(|e| format!("cannot run wevtutil: {e}"))?;
        if !out.status.success() {
            return Err(format!("wevtutil failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        parse_events_xml(&String::from_utf8_lossy(&out.stdout)).map_err(|e| e.to_string())
    }
}
