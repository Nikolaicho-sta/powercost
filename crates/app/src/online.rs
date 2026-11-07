//! Updating from GitHub Releases, for installs that have no project folder (everyone except the
//! developer). This is the ONLY update code that uses the network and it only runs when the user
//! presses "Check for updates"/"Update now", or turned on the automatic check in Settings (off by
//! default). Documented in docs/PRIVACY.md and D44.
//!
//! Flow: read the latest release's JSON, compare its version with ours, download the installer,
//! verify its SHA-256 against the checksum published with the release, then hand over to a hidden
//! PowerShell helper that waits for this app to exit, runs the installer silently over the existing
//! install and starts the app again.
//!
//! Integrity is checked, authenticity is not: the installer is not code-signed, so this protects
//! against a corrupted or truncated download, not against a compromised GitHub account.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const API_URL: &str = "https://api.github.com/repos/Nikolaicho-sta/powercost/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/Nikolaicho-sta/powercost/releases/download/";
const MAX_INSTALLER_BYTES: u64 = 60 * 1024 * 1024;
const INSTALLER_SUFFIX: &str = "_x64-setup.exe";

/// Test hook: `POWERCOST_UPDATE_TEST_API=http://127.0.0.1:PORT/latest.json` makes the app read that
/// "latest release" document instead of GitHub's and skip the download-URL allow-list. It exists so
/// the whole flow can be tested without publishing a release; it is not a user feature.
pub fn test_api() -> Option<String> {
    std::env::var("POWERCOST_UPDATE_TEST_API").ok().filter(|v| !v.trim().is_empty())
}

#[derive(Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
    /// GitHub reports "sha256:<hex>" for uploaded assets.
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub version: String,
    /// First line of the release notes (or its title), shown in the UI.
    pub subject: String,
    pub url: String,
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

pub fn parse_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The checksum for `asset`: GitHub's own `digest`, else the 64-hex value following the asset's
/// file name in the release notes.
pub fn expected_sha256(release: &Release, asset: &Asset) -> Option<String> {
    if let Some(d) = &asset.digest {
        let hex = d.trim().strip_prefix("sha256:").unwrap_or(d.trim());
        if is_sha256(hex) {
            return Some(hex.to_ascii_lowercase());
        }
    }
    let body = release.body.as_deref()?;
    let after = &body[body.find(&asset.name)? + asset.name.len()..];
    after
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|tok| is_sha256(tok))
        .map(|t| t.to_ascii_lowercase())
}

pub fn url_allowed(url: &str, test_mode: bool) -> bool {
    test_mode || url.starts_with(DOWNLOAD_PREFIX)
}

/// `Ok(None)`: nothing newer. `Err`: a newer release exists but cannot be installed safely.
pub fn evaluate(current: &str, release: &Release, test_mode: bool) -> Result<Option<Offer>, String> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let latest = parse_version(&release.tag_name).ok_or_else(|| format!("the latest release has an unreadable version ({})", release.tag_name))?;
    let current = parse_version(current).ok_or_else(|| format!("this app has an unreadable version ({current})"))?;
    if latest <= current {
        return Ok(None);
    }
    let asset = installer_asset(release).ok_or_else(|| format!("release {latest} has no Windows installer attached"))?;
    if !url_allowed(&asset.browser_download_url, test_mode) {
        return Err(format!("release {latest} points to an unexpected download address"));
    }
    let sha256 = expected_sha256(release, asset).ok_or_else(|| format!("release {latest} publishes no checksum, so the download cannot be verified"))?;
    // trim also removes a byte-order mark that some tools put at the start of the notes
    let clean = |s: &str| s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}').to_string();
    let subject = release.body.as_deref().and_then(|b| b.lines().map(clean).find(|l| !l.is_empty())).or_else(|| release.name.clone()).unwrap_or_default();
    Ok(Some(Offer { version: latest.to_string(), subject, url: asset.browser_download_url.clone(), name: asset.name.clone(), sha256, size: asset.size }))
}

pub fn installer_asset(release: &Release) -> Option<&Asset> {
    release.assets.iter().find(|a| a.name.ends_with(INSTALLER_SUFFIX))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).user_agent(concat!("PowerCost/", env!("CARGO_PKG_VERSION"))).build()
}

pub fn fetch_release() -> Result<Release, String> {
    let test = test_api();
    let url = test.as_deref().unwrap_or(API_URL);
    let text = agent()
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not reach the update server: {e}"))?
        .into_string()
        .map_err(|e| format!("unreadable answer from the update server: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("unexpected answer from the update server: {e}"))
}

/// Ask GitHub whether a newer release exists.
pub fn check() -> Result<Option<Offer>, String> {
    let release = fetch_release()?;
    evaluate(env!("CARGO_PKG_VERSION"), &release, test_api().is_some())
}

pub fn update_dir() -> PathBuf {
    std::env::temp_dir().join("PowerCost-update")
}

/// Download the installer and verify its SHA-256. Nothing is returned (and the file is deleted)
/// unless the checksum matches.
pub fn download(offer: &Offer, dir: &Path) -> Result<PathBuf, String> {
    if !offer.name.ends_with(INSTALLER_SUFFIX) || offer.name.contains(['/', '\\']) {
        return Err("refusing to download an unexpected file name".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let final_path = dir.join(&offer.name);
    let part = dir.join(format!("{}.part", offer.name));
    let resp = agent().get(&offer.url).call().map_err(|e| format!("download failed: {e}"))?;
    let mut reader = resp.into_reader().take(MAX_INSTALLER_BYTES + 1);
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_INSTALLER_BYTES {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err("the download is larger than expected; stopped".into());
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    drop(file);
    let got = format!("{:x}", hasher.finalize());
    if got != offer.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err("the downloaded installer does not match its published checksum, so it was discarded".into());
    }
    if total < 1024 {
        let _ = std::fs::remove_file(&part);
        return Err("the downloaded installer is too small to be valid".into());
    }
    let _ = std::fs::remove_file(&final_path);
    std::fs::rename(&part, &final_path).map_err(|e| e.to_string())?;
    Ok(final_path)
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The helper that finishes the update after this process has exited. Whatever happens to the
/// installer, the app is started again afterwards (the old copy if the install failed).
pub fn apply_script(pid: u32, installer: &Path, install_dir: &Path, log: &Path) -> String {
    let exe = install_dir.join("powercost.exe");
    format!(
        "$ErrorActionPreference = 'Continue'\r\n\
         $log = {log}\r\n\
         function Log($m) {{ Add-Content -LiteralPath $log -Value (\"$(Get-Date -Format s) $m\") }}\r\n\
         Log 'waiting for PowerCost to exit'\r\n\
         try {{ Wait-Process -Id {pid} -Timeout 60 -ErrorAction Stop }} catch {{ Log \"wait: $_\" }}\r\n\
         Log 'installing'\r\n\
         try {{ $p = Start-Process -FilePath {installer} -ArgumentList '/S', ('/D=' + {dir}) -Wait -PassThru; Log \"installer exit code $($p.ExitCode)\" }} catch {{ Log \"install failed: $_\" }}\r\n\
         if (Test-Path -LiteralPath {exe}) {{ Start-Process -FilePath {exe}; Log 'started' }} else {{ Log 'program not found after install' }}\r\n\
         Remove-Item -LiteralPath {installer} -ErrorAction SilentlyContinue\r\n",
        log = ps_quote(&log.display().to_string()),
        pid = pid,
        installer = ps_quote(&installer.display().to_string()),
        dir = ps_quote(&install_dir.display().to_string()),
        exe = ps_quote(&exe.display().to_string()),
    )
}

/// Write and start the helper (hidden, detached). The caller must exit the app right after.
pub fn start_helper(installer: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let install_dir = exe.parent().ok_or("cannot find the install folder")?.to_path_buf();
    let dir = update_dir();
    let script_path = dir.join("apply.ps1");
    let log = dir.join("apply.log");
    std::fs::write(&script_path, apply_script(std::process::id(), installer, &install_dir, &log)).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A hidden console of its own, in its own process group so it outlives this process. (Adding
        // DETACHED_PROCESS made powershell.exe not start at all, found by the end-to-end test.)
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
            .arg(&script_path)
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not start the installer helper: {e}"))
    }
    #[cfg(not(windows))]
    {
        let _ = script_path;
        Err("updating from the app is only available on Windows".into())
    }
}

/// Check, download, verify and start the helper. The caller exits the app when this returns Ok.
pub fn install_latest() -> Result<(), String> {
    let offer = check()?.ok_or("you already have the newest version")?;
    let installer = download(&offer, &update_dir())?;
    start_helper(&installer)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "5d8fd897171b1d3baba6dba4fb63710595681392a9651437885197b179372f08";

    fn release(tag: &str, asset: &str, digest: Option<&str>, body: &str) -> Release {
        Release {
            tag_name: tag.into(),
            name: Some("PowerCost".into()),
            body: Some(body.into()),
            draft: false,
            prerelease: false,
            assets: vec![Asset { name: asset.into(), browser_download_url: format!("{DOWNLOAD_PREFIX}{tag}/{asset}"), size: 3_000_000, digest: digest.map(str::to_string) }],
        }
    }

    #[test]
    fn versions_parse_with_or_without_v() {
        assert_eq!(parse_version("v0.2.0"), semver::Version::parse("0.2.0").ok());
        assert_eq!(parse_version("0.10.1"), semver::Version::parse("0.10.1").ok());
        assert!(parse_version("latest").is_none());
    }

    #[test]
    fn newer_release_is_offered_older_or_equal_is_not() {
        let r = release("v0.2.0", "PowerCost_0.2.0_x64-setup.exe", Some(&format!("sha256:{HEX}")), "Faster\nmore");
        let offer = evaluate("0.1.0", &r, false).unwrap().unwrap();
        assert_eq!(offer.version, "0.2.0");
        assert_eq!(offer.subject, "Faster");
        assert_eq!(offer.sha256, HEX);
        assert!(evaluate("0.2.0", &r, false).unwrap().is_none());
        assert!(evaluate("0.10.0", &r, false).unwrap().is_none(), "0.10 is newer than 0.2, not older");
        assert!(evaluate("0.1.9", &release("v0.1.10", "x_x64-setup.exe", Some(HEX), ""), false).unwrap().is_some(), "0.1.10 is newer than 0.1.9");
    }

    #[test]
    fn a_byte_order_mark_in_the_notes_does_not_reach_the_summary() {
        let r = release("v0.2.0", "a_x64-setup.exe", Some(HEX), "\u{feff}Summary line\r\nmore");
        assert_eq!(evaluate("0.1.0", &r, false).unwrap().unwrap().subject, "Summary line");
    }

    #[test]
    fn drafts_and_prereleases_are_ignored() {
        let mut r = release("v9.0.0", "a_x64-setup.exe", Some(HEX), "");
        r.prerelease = true;
        assert!(evaluate("0.1.0", &r, false).unwrap().is_none());
        r.prerelease = false;
        r.draft = true;
        assert!(evaluate("0.1.0", &r, false).unwrap().is_none());
    }

    #[test]
    fn checksum_comes_from_the_digest_or_from_the_release_notes() {
        let name = "PowerCost_0.2.0_x64-setup.exe";
        let a = release("v0.2.0", name, Some(&format!("sha256:{}", HEX.to_uppercase())), "");
        assert_eq!(expected_sha256(&a, &a.assets[0]).as_deref(), Some(HEX));
        let notes = format!("SHA-256 of `{name}`:\n`{HEX}`\n");
        let b = release("v0.2.0", name, None, &notes);
        assert_eq!(expected_sha256(&b, &b.assets[0]).as_deref(), Some(HEX));
        // a hash that belongs to another file name must not be used
        let c = release("v0.2.0", name, None, &format!("SHA-256 of other.exe: {HEX}"));
        assert_eq!(expected_sha256(&c, &c.assets[0]), None);
        let d = release("v0.2.0", name, Some("sha256:abc"), "no checksum here");
        assert_eq!(expected_sha256(&d, &d.assets[0]), None);
    }

    #[test]
    fn a_release_without_checksum_or_installer_is_an_error_not_an_update() {
        let no_sum = release("v0.2.0", "PowerCost_0.2.0_x64-setup.exe", None, "nothing");
        assert!(evaluate("0.1.0", &no_sum, false).is_err());
        let no_installer = release("v0.2.0", "notes.txt", Some(HEX), "");
        assert!(evaluate("0.1.0", &no_installer, false).is_err());
    }

    #[test]
    fn only_this_projects_release_downloads_are_allowed() {
        assert!(url_allowed("https://github.com/Nikolaicho-sta/powercost/releases/download/v0.2.0/a_x64-setup.exe", false));
        assert!(!url_allowed("https://evil.example/powercost/a_x64-setup.exe", false));
        assert!(!url_allowed("https://github.com/someone-else/powercost/releases/download/v1/a_x64-setup.exe", false));
        assert!(url_allowed("http://127.0.0.1:9/a_x64-setup.exe", true));
        let mut r = release("v0.2.0", "a_x64-setup.exe", Some(HEX), "");
        r.assets[0].browser_download_url = "https://evil.example/a_x64-setup.exe".into();
        assert!(evaluate("0.1.0", &r, false).is_err());
    }

    #[test]
    fn real_github_json_shape_parses() {
        let json = r#"{"tag_name":"v0.2.0","name":"PowerCost 0.2.0","draft":false,"prerelease":false,"body":"Notes","extra":1,
            "assets":[{"name":"PowerCost_0.2.0_x64-setup.exe","size":3597834,"browser_download_url":"https://github.com/Nikolaicho-sta/powercost/releases/download/v0.2.0/PowerCost_0.2.0_x64-setup.exe","digest":"sha256:5d8fd897171b1d3baba6dba4fb63710595681392a9651437885197b179372f08","download_count":3}]}"#;
        let r: Release = serde_json::from_str(json).unwrap();
        assert!(evaluate("0.1.0", &r, false).unwrap().is_some());
    }

    #[test]
    fn download_rejects_bad_names_and_wrong_checksums() {
        let dir = std::env::temp_dir().join(format!("pc-online-test-{}", std::process::id()));
        let bad = Offer { version: "0.2.0".into(), subject: String::new(), url: "http://127.0.0.1:1/x".into(), name: "..\\evil.exe".into(), sha256: HEX.into(), size: 0 };
        assert!(download(&bad, &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_script_waits_installs_with_explicit_folder_and_restarts() {
        let s = apply_script(4242, Path::new(r"C:\Temp\PowerCost-update\a_x64-setup.exe"), Path::new(r"C:\Users\O'Neil\AppData\Local\PowerCost"), Path::new(r"C:\Temp\apply.log"));
        assert!(s.contains("Wait-Process -Id 4242"));
        assert!(s.contains("'/S'") && s.contains("'/D=' +"));
        assert!(s.contains(r"'C:\Users\O''Neil\AppData\Local\PowerCost'"), "single quotes are doubled");
        assert!(s.contains(r"powercost.exe") && s.contains("Start-Process -FilePath '"));
    }
}
