# PowerCost: handoff notes (state at M24, 2026-11-08)

Read `CLAUDE.md` (principles), then this file. Reasoning for every major choice is in `docs/DECISIONS.md` (D1 to D46; D43 to D46 are newest and sit at the top of their group in reverse order). Network use is documented in `docs/PRIVACY.md`.

## What it is
Windows desktop app (Tauri 2, Rust workspace, React/TS UI in `ui/`, SQLite) that estimates what a PC costs in electricity, split by power state (active / idle / sleep / off) and tariff period. Bulgarian Electrohold presets first. Every number is an estimate with a method and confidence label. Crate list: see "Project layout" in `README.md`.

## Where things stand
Milestones M0 to M24 are done, committed and pushed to `origin/main` (public repo `Nikolaicho-sta/powercost`).
- M0 to M6: engines, sensors, estimator, storage, tray tracker, dashboard, settings, installer.
- M7 stopwatch, M8 browsing earlier periods.
- M9 to M10, M18 to M20: one-click local update (`Update PowerCost.vbs`, `scripts/update.ps1`, `scripts/update-ui.ps1`; fast path replaces the exe instead of running an installer).
- M11 to M13: download site `site/index.html`, live at https://power-cost.vercel.app (Vercel project `powercost`, deployed through the Vercel MCP with inline files, not git).
- M14 to M17: updates for other users through GitHub Releases (`crates/app/src/online.rs`), opt-in, SHA-256 verified. Old releases v0.1.0, v0.2.0, v0.2.1 use the retired semver scheme (D48); from M27 on releases are named M<n>.
- M21 to M23: `scripts/fresh-pc-test.wsb`, a Windows Sandbox config for testing as a new user (the sandbox has no browser; it downloads the installers with PowerShell).
- M24: `scripts/release.ps1` puts the newest commit's M label in the release notes and title.

**Unreleased:** M18 and later are not in any published release (latest is v0.2.1, built at M17). Publishing = commit (subject `M<n> ...`), then `scripts/release.ps1 -Notes "one line"`. Publishing is public: ask the user first.

## Update system (two modes, `updater::mode()`)
- **Folder:** the build still sits next to the project folder with `scripts/update.ps1`. Compares the local `git log` with the embedded commit; no network.
- **Github:** installed copy without a project folder. Reads `releases/latest`, compares the M number in the tag (M27 = 27) with the one embedded at build time, downloads the `*_x64-setup.exe` asset, verifies SHA-256 (from the asset digest or release notes), runs a helper script that installs silently and restarts.
- The daily notification-only check (`online_update_check`) is ON by default since M26 (D47, user decision); downloading/installing still needs the "Update now" button. Users only get a notification for releases that are PUBLISHED (version bumped in both manifests + `scripts/release.ps1`), not for commits.
- Test hook: `POWERCOST_UPDATE_TEST_API=<url>` fakes the release feed.

## Conventions the user set
- **Commit subjects are `M<n> <what was added>`** (next is M25), e.g. `M7 Stopwatch`. Body describes the change and ends with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. This overrides conventional commits. Write the message to a file and use `git commit -F` (multi-line `-m` got mangled in PowerShell).
- Add files to commits explicitly. `PowerCost/` at the repo root is a stray nested clone: do not touch or add it.
- Never touch the user's real `%APPDATA%\PowerCost` or their running app. Tests use `--sim`, a throwaway `APPDATA` and `WEBVIEW2_USER_DATA_FOLDER`. A second launch is forwarded to the running copy (single instance), so quit the real one or use `--sim`.
- Never kill processes you did not start; use exact PIDs.
- Downloads, installs and publishing need the user's explicit OK.
- PowerShell 5.1: `Set-Content -Encoding utf8` writes a BOM (use `[IO.File]::WriteAllText` with `UTF8Encoding($false)`); foreground `Start-Sleep` is blocked; prefer the Edit tool over `-replace` for source edits.

## Build and test
- `cargo test` (workspace default members skip the Tauri crate). Building `crates/app` needs `npm run build` in `ui/` first. `cargo clippy --workspace --all-targets`; `npm test` in `ui/`.
- Installer: `crates/app` then `..\..\ui\node_modules\.bin\tauri build` (output in `target/release/bundle/nsis/`).
- Quick local build used by the updater: `cargo build --release -p powercost --features tauri/custom-protocol`.
- Try the UI with demo data: `powercost.exe --sim`. See what a brand-new user sees (empty database, setup wizard): `powercost.exe --sim --first-run`; add `POWERCOST_UPDATE_TEST_API` to show the GitHub-mode update checkbox. Both are safe next to the real app.

## Reference machine
User's laptop: Ryzen 7 8845HS, RTX 4060, Windows 11 (Modern Standby only), Bulgarian locale, Electrohold customer. A Tapo P100 plug exists but cannot measure power (P110/P115 can). Tracker budget: about 25 MB working set and about 0.01% CPU measured.

## Open items (none requested yet)
- (Done) Publish the first M-numbered release; done: M27 published, old v0.x releases deleted, sandbox downloads the latest only.
- Real in-app GitHub update 0.2.0 to 0.2.1 never run end to end on a real install; the sandbox file exists for this. The new progress window has not been run on the user's own installed copy.
- No LICENSE file (manifests say MIT; not confirmed with the user). Installer is unsigned (SmartScreen warns).
- Commits publicly show the author name and email `niki.hristov.junior@gmail.com`.
- EVN and Energo-Pro presets unverified; the EWRC decision Ц-13 PDF was never read; hibernate/crash classification only tested on fixtures; first-run IP lookup never called for real.
- Smart-plug sources are an interface only (`sources-api`); a P110/P115 Tapo source was discussed, not built.
