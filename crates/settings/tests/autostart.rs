//! Autostart against a THROWAWAY registry key; the real Run key is never touched.
#![cfg(windows)]

use settings::autostart::RunKey;
use std::path::Path;

fn test_key() -> RunKey {
    RunKey { key_path: r"HKCU\Software\PowerCostTest\Run".into(), value_name: "PowerCostTest".into() }
}

fn cleanup() {
    let _ = std::process::Command::new("reg").args(["delete", r"HKCU\Software\PowerCostTest", "/f"]).output();
}

#[test]
fn enable_disable_roundtrip_with_quoted_path_and_args() {
    cleanup();
    let k = test_key();
    assert!(!k.is_enabled());
    assert!(k.command().is_none());
    k.disable().expect("disabling something that is not there is fine");

    let exe = Path::new(r"C:\Program Files\PowerCost\powercost.exe");
    k.enable(exe, "--tray").unwrap();
    assert!(k.is_enabled());
    let cmd = k.command().expect("command registered");
    assert_eq!(cmd, r#""C:\Program Files\PowerCost\powercost.exe" --tray"#, "path with spaces must be quoted");

    // enabling again overwrites instead of failing
    k.enable(Path::new(r"C:\Other\powercost.exe"), "--tray").unwrap();
    assert!(k.command().unwrap().contains(r"C:\Other\powercost.exe"));

    k.disable().unwrap();
    assert!(!k.is_enabled());
    cleanup();
}

#[test]
fn the_real_startup_key_name_is_the_per_user_run_key() {
    let k = RunKey::windows_default();
    assert!(k.key_path.starts_with("HKCU\\"), "per-user, no admin rights");
    assert_eq!(k.value_name, "PowerCost");
}