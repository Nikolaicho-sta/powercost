use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn main() {
    // A label like "M8 01fbf4a" (latest commit subject + short hash), plus "+edits" when the working
    // tree has uncommitted changes. Shown in Settings so you can tell which build you are running.
    let label = match (git(&["log", "-1", "--format=%s"]), git(&["rev-parse", "--short", "HEAD"])) {
        (Some(subject), Some(hash)) => {
            let subject: String = subject.chars().take(24).collect();
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
            format!("{subject} {hash}{}", if dirty { " +edits" } else { "" })
        }
        _ => "unknown build".to_string(),
    };
    println!("cargo:rustc-env=POWERCOST_BUILD={label}");
    // The full commit and the project folder this build came from. The app compares them with the
    // folder's newest commit to tell you an update is ready (a local check, no network).
    println!("cargo:rustc-env=POWERCOST_COMMIT={}", git(&["rev-parse", "HEAD"]).unwrap_or_default());
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let source = manifest.join("..").join("..").canonicalize().map(|p| p.display().to_string()).unwrap_or_default();
    let source = source.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(source);
    println!("cargo:rustc-env=POWERCOST_SOURCE={source}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    tauri_build::build()
}
