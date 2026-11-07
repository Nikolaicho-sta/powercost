//! Update check and one-click update. Local only: the app compares the commit it was built from
//! with the newest commit of the project folder it was built in (`git log`, no network). Updating
//! runs `scripts/update.ps1`, which quits this app cleanly, rebuilds, installs over it and restarts.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BUILT_COMMIT: &str = env!("POWERCOST_COMMIT");
pub const SOURCE_DIR: &str = env!("POWERCOST_SOURCE");

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateStatus {
    /// Not checked yet.
    #[default]
    Unknown,
    /// The project folder this build came from is missing, so there is nothing to update from.
    Unavailable { reason: String },
    UpToDate,
    Available { hash: String, subject: String },
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

/// Pure decision, separated so it can be tested: `latest` is `(hash, subject)` of the folder's HEAD.
pub fn decide(built: &str, latest: Option<(String, String)>) -> UpdateStatus {
    match latest {
        None => UpdateStatus::Unavailable { reason: "could not read the project folder's latest commit (is git installed?)".into() },
        Some((hash, subject)) => {
            if built.is_empty() {
                UpdateStatus::Unavailable { reason: "this build does not know which commit it came from".into() }
            } else if hash == built {
                UpdateStatus::UpToDate
            } else {
                UpdateStatus::Available { hash, subject }
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

pub fn check() -> UpdateStatus {
    let source = Path::new(SOURCE_DIR);
    if SOURCE_DIR.is_empty() || !script_path(source).is_file() {
        return UpdateStatus::Unavailable { reason: format!("the project folder this app was built from was not found ({SOURCE_DIR})") };
    }
    decide(BUILT_COMMIT, latest_commit(source))
}

/// Start the update script in its own console window (so the user can watch it) and return at once.
/// The script quits this app itself, after the build, so the app keeps tracking meanwhile.
pub fn start() -> Result<(), String> {
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
        assert_eq!(decide("abc", latest("def")), UpdateStatus::Available { hash: "def".into(), subject: "M10".into() });
    }

    #[test]
    fn missing_information_is_unavailable_not_an_update() {
        assert!(matches!(decide("abc", None), UpdateStatus::Unavailable { .. }));
        assert!(matches!(decide("", latest("def")), UpdateStatus::Unavailable { .. }));
    }

    #[test]
    fn real_check_does_not_panic() {
        // In a checkout this is UpToDate or Available; elsewhere Unavailable. Never a panic.
        eprintln!("update check in this checkout: {:?}", check());
    }
}
