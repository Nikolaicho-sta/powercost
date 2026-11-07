//! Update check and one-click update, in two modes:
//!
//! * **Folder** (the developer's PC): the app was built from a project folder that still exists. It
//!   compares its own commit with the folder's newest commit (`git log`, no network) and updates by
//!   running `scripts/update.ps1` (rebuild, install over, restart).
//! * **Github** (everyone else): there is no project folder, so it looks at GitHub Releases, downloads
//!   the installer, verifies its checksum and installs it (see `online.rs`). This is the only
//!   network use and it is opt-in.

use crate::online;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BUILT_COMMIT: &str = env!("POWERCOST_COMMIT");
pub const SOURCE_DIR: &str = env!("POWERCOST_SOURCE");

#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSource {
    Folder,
    Github,
}

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateStatus {
    /// Not checked yet.
    #[default]
    Unknown,
    /// Nothing to update from, or the check failed; `reason` says why.
    Unavailable { reason: String },
    UpToDate,
    /// `id` identifies the update (commit hash or version) so it is announced once; `label` is for display.
    Available { source: UpdateSource, id: String, label: String, subject: String },
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn hide_window(_: &mut Command) {}

fn script_path(source: &Path) -> PathBuf {
    source.join("scripts").join("update.ps1")
}

fn folder_usable() -> bool {
    !SOURCE_DIR.is_empty() && script_path(Path::new(SOURCE_DIR)).is_file()
}

/// Which mode this installation uses. The test hook forces GitHub mode.
pub fn mode() -> UpdateSource {
    if online::test_api().is_none() && folder_usable() {
        UpdateSource::Folder
    } else {
        UpdateSource::Github
    }
}

/// Pure decision for folder mode, separated so it can be tested: `latest` is `(hash, subject)` of HEAD.
pub fn decide(built: &str, latest: Option<(String, String)>) -> UpdateStatus {
    match latest {
        None => UpdateStatus::Unavailable { reason: "could not read the project folder's latest commit (is git installed?)".into() },
        Some((hash, subject)) => {
            if built.is_empty() {
                UpdateStatus::Unavailable { reason: "this build does not know which commit it came from".into() }
            } else if hash == built {
                UpdateStatus::UpToDate
            } else {
                UpdateStatus::Available { source: UpdateSource::Folder, label: hash.chars().take(7).collect(), id: hash, subject }
            }
        }
    }
}

fn latest_commit(source: &Path) -> Option<(String, String)> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(source).args(["log", "-1", "--format=%H%n%s"]);
    hide_window(&mut cmd);
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines = text.lines();
    let hash = lines.next()?.trim().to_string();
    let subject = lines.next().unwrap_or("").trim().to_string();
    (!hash.is_empty()).then_some((hash, subject))
}

/// Check now. Folder mode is a local `git log`; GitHub mode makes a request to github.com, so callers
/// only use it for a button press or when the user turned on the automatic check.
pub fn check() -> UpdateStatus {
    match mode() {
        UpdateSource::Folder => decide(BUILT_COMMIT, latest_commit(Path::new(SOURCE_DIR))),
        UpdateSource::Github => match online::check() {
            Ok(None) => UpdateStatus::UpToDate,
            Ok(Some(o)) => UpdateStatus::Available { source: UpdateSource::Github, id: o.version.clone(), label: format!("version {}", o.version), subject: o.subject },
            Err(reason) => UpdateStatus::Unavailable { reason },
        },
    }
}

/// Folder mode: run the update script in its own console window (so the user can watch it) and
/// return at once. The script quits this app itself, after the build.
pub fn start_folder() -> Result<(), String> {
    let source = Path::new(SOURCE_DIR);
    let script = script_path(source);
    if !script.is_file() {
        return Err(format!("the update script was not found: {}", script.display()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .current_dir(source)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not start the update: {e}"))
    }
    #[cfg(not(windows))]
    {
        Err("updating from the app is only available on Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest(h: &str) -> Option<(String, String)> {
        Some((h.to_string(), "M10".to_string()))
    }

    #[test]
    fn same_commit_is_up_to_date() {
        assert_eq!(decide("abc", latest("abc")), UpdateStatus::UpToDate);
    }

    #[test]
    fn different_commit_is_an_update() {
        assert_eq!(
            decide("abc", latest("def1234567")),
            UpdateStatus::Available { source: UpdateSource::Folder, id: "def1234567".into(), label: "def1234".into(), subject: "M10".into() }
        );
    }

    #[test]
    fn missing_information_is_unavailable_not_an_update() {
        assert!(matches!(decide("abc", None), UpdateStatus::Unavailable { .. }));
        assert!(matches!(decide("", latest("def")), UpdateStatus::Unavailable { .. }));
    }

    #[test]
    fn status_serialises_for_the_ui() {
        let s = serde_json::to_value(UpdateStatus::Available { source: UpdateSource::Github, id: "0.2.0".into(), label: "version 0.2.0".into(), subject: "x".into() }).unwrap();
        assert_eq!(s["state"], "available");
        assert_eq!(s["source"], "github");
    }
}
