//! Start with the OS. Windows: a value in the per-user `Run` key (no admin rights). The key path
//! is a parameter so tests use a throwaway key instead of the real startup entries.

use std::path::Path;

#[derive(Debug, Clone)]
pub struct RunKey {
    /// e.g. `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
    pub key_path: String,
    pub value_name: String,
}

impl RunKey {
    pub fn windows_default() -> Self {
        Self { key_path: r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run".into(), value_name: "PowerCost".into() }
    }

    #[cfg(windows)]
    fn reg(&self, args: &[&str]) -> Result<std::process::Output, String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("reg").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| format!("cannot run reg.exe: {e}"))
    }

    /// Register `exe` with `args` (typically `--tray` so the dashboard window stays closed at login).
    #[cfg(windows)]
    pub fn enable(&self, exe: &Path, args: &str) -> Result<(), String> {
        let command = format!("\"{}\" {}", exe.display(), args).trim().to_string();
        let out = self.reg(&["add", &self.key_path, "/v", &self.value_name, "/t", "REG_SZ", "/d", &command, "/f"])?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    /// Removing a value that is not there is not an error.
    #[cfg(windows)]
    pub fn disable(&self) -> Result<(), String> {
        if !self.is_enabled() {
            return Ok(());
        }
        let out = self.reg(&["delete", &self.key_path, "/v", &self.value_name, "/f"])?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    #[cfg(windows)]
    pub fn is_enabled(&self) -> bool {
        self.reg(&["query", &self.key_path, "/v", &self.value_name]).map(|o| o.status.success()).unwrap_or(false)
    }

    /// The registered command line, if any.
    #[cfg(windows)]
    pub fn command(&self) -> Option<String> {
        let out = self.reg(&["query", &self.key_path, "/v", &self.value_name]).ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.lines().find(|l| l.contains("REG_SZ")).and_then(|l| l.split("REG_SZ").nth(1)).map(|s| s.trim().to_string())
    }

    #[cfg(not(windows))]
    pub fn enable(&self, _exe: &Path, _args: &str) -> Result<(), String> {
        Err("start with the OS is only implemented for Windows".into())
    }
    #[cfg(not(windows))]
    pub fn disable(&self) -> Result<(), String> {
        Ok(())
    }
    #[cfg(not(windows))]
    pub fn is_enabled(&self) -> bool {
        false
    }
    #[cfg(not(windows))]
    pub fn command(&self) -> Option<String> {
        None
    }
}
