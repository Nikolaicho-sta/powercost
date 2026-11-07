# Decisions

Reasoning behind the major choices. Newest decisions at the bottom.

## D1 - Tauri 2 + Rust + React/TS + SQLite
Resident tracker must stay under 50 MB RAM / 1% CPU. Rust core has no GC and the WebView window can be destroyed while minimised to tray. Electron and Python+Qt were rejected on memory. Plan approved by the user.

## D2 - Schedules in local wall-clock time, ranges never wrap midnight
Each UTC instant is converted to the tariff's IANA timezone, so DST needs no special code (spring day = 23 h, autumn day = 25 h, tested). Ranges must have start < end (`24:00` allowed as end); an overnight window is written as two ranges (22:00-24:00, 00:00-06:00). This removes the ambiguity of which day's rules own the after-midnight part. Validation requires every minute of every season x day type to be covered exactly once, and every calendar day to be in exactly one season.

## D3 - Decimal strings in tariff JSON
Money values are JSON strings ("0.09297"), never numbers, so no float ever touches a price. The schema enforces it.

## D4 - Fixed-point storage: nano-units, not micro
Plan said 6 decimals; a 1-minute interval at ~60 W costs ~0.0002 EUR, so micro-units would add ~0.3% random error per interval. Energy and cost are stored as integer nano-units (1e-9). Pricing is exact in `Decimal` and rounded half-even once per stored interval; measured drift is ~0.33 nano per interval (about 0.0002 kWh/year of continuous use), negligible. Rounding to cents happens only for display.

## D5 - Tariff versions over time
`TariffSet::at` picks the tariff with the latest `valid_from` on or before the local date. An expired tariff keeps being used until replaced (best available estimate; UI warns). Each stored interval records tariff id and version so history can be re-derived.

## D6 - Bulgarian seasons are calendar dates, not DST dates
Electrohold's official page defines night hours by fixed dates (01.04-31.10 night 23:00-07:00; 01.11-31.03 night 22:00-06:00). The preset follows that literally; the DST switch on the last Sunday of March/October does not move the schedule.

## D7 - Verification status is data
Every tariff carries `verification` (verified / partial / unverified / example) and `last_verified`; the engine emits warnings so the UI can badge unverified presets. EVN and Energo-Pro presets are not fully verified (see notes in the files).

## D8 - Event log via `wevtutil.exe`, not the Win32 Evt* API
Gaps are classified once at startup/resume, so spawning the built-in `wevtutil qe System /f:xml` is cheap, needs no admin rights and no unsafe FFI. The XML is parsed with `roxmltree` and the same parser reads the sanitised real-log fixtures. Revisit (EvtQuery) only if startup latency or localisation causes problems.

## D9 - Modern Standby "sleep" is mostly active on the reference laptop
Measured on the reference laptop (Ryzen 7 8845HS, Windows 11, S0 Low Power Idle only - no S3): over 4 days, 85 h were logged as Modern Standby but Kernel-Power 507 reports 96-98% *active residency* (CPU not in the low-power state; lid closed, external monitor, audio). A flat "sleep = ~1 W" would badly under-cost it. Gap segments therefore carry `SleepDetails` (duration, active time, AC/battery, battery delta) and the M3 estimator must price the active share with a running-system model and only the remainder at sleep watts. Open: what power the active share really draws (needs a wall meter).

## D10 - Heartbeats are the source of truth; events only explain gaps
Events fill heartbeat gaps; where the tracker kept running (even during Modern Standby) heartbeats are used as-is. Segments always tile the gap exactly (tested invariant). 506/507 pairs 1 ms apart and wake blips <= 10 s fold into one sleep. A sleep that began before the gap is scaled to the part inside it.

## D11 - The System log keeps only ~5 days
The reference machine's System log starts 5 days back. The app must therefore copy relevant power events into its own database at each start/resume (and periodically); a gap longer than the log's retention is otherwise `Unknown` (configurable assumption, default Off).

## D12 - Crash/hibernate semantics are documented, not yet observed
No crash (Kernel-Power 41, EventLog 6008, Kernel-Boot 20 LastShutdownGood=false) or hibernate (Kernel-Power 42 TargetState 4, Kernel-Boot 27 BootType 2) occurred on the reference machine in the available log. Their fixtures are synthetic and labelled as such. A user-run hibernate test on the laptop is proposed before M2 is considered fully verified for hibernate.

## D13 - Estimator: calibration wins over partial sensors by default
The spec orders the chain sensors -> calibrated -> default. CPU+GPU sensors measure only part of the wall draw (rest of system is modelled), while a wall-meter calibration measures the whole PC, so `prefer_calibrated` (default true) puts Calibrated first when a calibration exists; set it to false for the literal spec order. Every estimate reports its layer, a +/- range (sensors 15%, calibrated 5%, default 30%) and a method string for the UI tooltip. Calibrated utilisation = the busier of CPU and GPU load, interpolated piecewise-linearly (idle and full required, mid points optional). Monitor power is counted only while ACTIVE (monitors sleep when idle) and is off by default.

## D14 - Modern Standby priced by its active share
`watts = idle_running * p + sleep_w * (1 - p)` with p = ActiveResidency / Duration from Kernel-Power 507 (see D9). With the reference laptop's ~96% active share a "sleep" costs ~96% of an idle PC. The idle figure comes from calibration if present, else the default model; the real draw of that active share is still unmeasured.

## D15 - Spans are the source of truth, minute rows are derived
`usage_span` stores constant-power spans (UTC seconds) together with the estimator `Inputs`, so history can be re-estimated after a calibration change (`reestimate`) and re-priced after a tariff correction (`recompute_costs`). `usage_cost` holds 1-minute priced intervals (PK minute+state+billable, partial minutes sum). Long nights are one span, so a gap costs one row, not 1,000.

## D16 - Compaction by whole local days
`compact(before, tz)` aggregates everything before the local midnight preceding `before` into `agg_hourly` (UTC hours) and `agg_daily` (local date, grouped in Rust so 23/25-hour DST days are right), then deletes those minute rows and the spans that ended before the cutoff. Totals are reproduced exactly (integer sums). Afterwards that history cannot be recomputed (`recompute_costs` reports `clipped_by_compaction`); spans straddling the cutoff survive. Default retention for the minute rows: 90 days. Known limit: hourly buckets are UTC hours, so zones with 30-minute offsets attribute up to 30 minutes at day edges.

## D17 - Only mains energy is billed
Spans carry `billable`; battery periods keep their kWh but cost 0. Sleep gaps use the on/off-AC flag from the 507 event; other gap kinds use the last heartbeat's power source.

## D18 - The dashboard read model lives in Rust; the UI only displays
`dashboard` computes period ranges (Monday-start weeks, local-midnight days, DST-safe), breakdowns, bucketed timelines (hour/day/month, padded with empty future buckets), the confidence mix and what-if. Amounts travel as integers (`kwh_nano`, `cost_nano`) plus a Rust-rounded `cost_minor`; the UI divides by 1e9 only to format. No money is summed or rounded in TypeScript.

## D19 - Closing the window destroys it; the tracker lives in the tray
To keep the resident footprint to the tracker, closing the dashboard destroys the WebView window (not hide) and `RunEvent::ExitRequested` without an exit code is prevented. Tray left-click or "Open dashboard" recreates it; "Quit" exits, joins the tracker thread (flushes the open minute, final heartbeat). `--tray` starts without a window; `--sim` runs on a generated database with no live tracker.

## D20 - Tracker semantics
Sampling every 10 s, one span per minute (averaged inputs, majority state), spans always tile (the first span after startup/gap starts where the previous coverage ended). Heartbeat every 30 s. Power events are copied to the database every 30 min and at startup. A gap seen *inside* the running process (clock jump > max(3 x interval, 90 s)) is classified from events; with no events it is a stall and is priced as Idle, not Off. A gap left from a previous run with no events uses the configurable Off assumption.

## D21 - What-if method
Computed from stored minute rows with the prices that actually applied. "Shut down at night": Idle/Sleep minutes inside the local night window are re-priced at the configured off watts. "Shift": a chosen share of ACTIVE energy outside the cheapest period of the current tariff is re-priced at that period's price (same kWh). Only billable, non-compacted history is used; a note says when part of the range is compacted. Yearly figures are a linear extrapolation of the window.

## D22 - Chart colours
States use categorical slots 1-5 in fixed order (Active blue, Idle orange, Sleep aqua, Hibernate yellow, Off magenta); tariff periods use slots 6-8 so a period never looks like a state; the estimate-confidence bar is an ordinal one-hue blue ramp. All sets were run through the palette validator in light and dark (adjacent CVD dE >= 8.4, normal-vision >= 19). Three light-mode slots are below 3:1 contrast, so every chart has a visible legend, a hover tooltip and a table view.

## D23 - Build layout
The Tauri crate is a workspace member but not a default member: plain `cargo test` / `cargo build` skip it and need no frontend bundle. Building the app needs `npm run build` in `ui/` first (the bundle is embedded). `export-mock` writes real dashboard JSON from simulated data so the UI can be previewed in a plain browser.
## D24 - Tariff catalogue: bundled presets are immutable, user tariffs are files
Presets are compiled in; a user's tariffs are plain JSON files `<id>.v<version>.json` in `%APPDATA%\PowerCost\tariffs` (shareable, hand-editable; a country is added by dropping a file there, no code change). Bundled (id, version) pairs cannot be overwritten or deleted - the editor offers "Copy to edit" (new `user-...` id, marked unverified). Files that fail validation are listed, never fatal. A price change is a NEW VERSION of the same id with a later `valid_from`; pricing history uses all versions of the selected id. Saving a version of the active tariff reprices the detailed history from its `valid_from`; compacted days keep their old prices and the UI says so.

## D25 - Region detection and the optional IP lookup
Region is guessed locally: the timezone wins over the language (it is physical, the locale is a preference) and both are offered; the user always confirms. The only network call in the product is the opt-in IP lookup (`ipapi.co`), made by the UI after an explicit button press, allow-listed in the CSP and parsed defensively (only a 2-letter country code and a sanitised timezone are accepted). Documented in `docs/PRIVACY.md`.

## D26 - Settings model and live apply
Settings are one JSON document in the database with a default for every field (old documents keep loading), validated as a whole (all problems reported at once; an invalid document is never written). Saving bumps a version in shared state; the tracker thread notices it and swaps estimator, tariff versions, thresholds and gap assumptions without a restart.

## D27 - Calibration wizard
Plain instructions (wattmeter placement, laptop battery full, monitor excluded), idle + full + optional half-load point, a preview of the resulting curve, then the last N days (default 30) are re-estimated from the stored inputs. Decimal commas are accepted everywhere and normalised to dots (Bulgarian users type "24,5").

## D28 - Budget alerts
Evaluated by the tracker thread once a minute from the stored month-to-date cost (plus fixed fees if they are included). Each threshold fires once per calendar month; a jump past several thresholds gives one notification for the highest and marks all of them. The dashboard meter shows severity with a text label, not colour alone.

## D29 - Exports
CSV/JSON per hour or local day, split by state and tariff period, from minute rows plus compacted aggregates (so compaction never changes an export). Amounts are exact 6-decimal strings derived from the integer nano-units. JSON also carries the tariff, the estimate-method mix and a disclaimer. The path comes from the system save dialog; the command only writes `.csv`/`.json`.

## D30 - Start with Windows
A per-user `Run` registry value (`"<exe>" --tray`) written through `reg.exe`: no admin rights, no extra dependency. Tested against a throwaway key, never the real one; the app does not touch it in `--sim` mode.

## D31 - Retention is now actually enforced
The tracker thread compacts detail older than the retention setting (default 90 days) at start and every 6 hours, and prunes old heartbeats. Until M5 compaction existed but nothing called it.

## D32 - Fixed monthly fees
Off by default. When included, each local day carries 1/(days in that month) of the VAT-inclusive monthly amount, shown as its own line and in the grand total; energy figures are unchanged.
## D33 - Installer: per-user NSIS
`tauri build` (run from `crates/app`) produces `PowerCost_<version>_x64-setup.exe` (about 3.4 MB): per-user install (no admin), English and Bulgarian, WebView2 bootstrapper for Windows 10 machines without it. An installer hook deletes the per-user "Start with Windows" Run value on uninstall; user data in `%APPDATA%\PowerCost` is deliberately left behind (the README says how to remove it). The first build downloads NSIS 3.13 and the Tauri NSIS utils DLL into `%LOCALAPPDATA%\tauri`. Tested by a silent install into a temp folder, a launch, an uninstall, and a check that the program, uninstall entry, shortcut and startup value are gone.

## D34 - Smart-plug sources: interface only
`sources-api` defines `WallPowerSource` (id, kind, read, max age), plausibility checks and a registry that returns the first healthy fresh reading and reports the problems of sources tried before it. Nothing implements it and the estimator does not consume it yet; the planned integration is a new top layer ("wall measurement") above sensors, with its own confidence label. This closes a gap from the original spec that M3 skipped. Implementations must stay opt-in and local-network and be documented in docs/PRIVACY.md first.

## D35 - Single instance
A second launch (login autostart plus a manual start, or a double click) focuses the running instance instead of starting a second tracker on the same database. Not active in `--sim` mode so a demo can run beside the real app. Verified: one process, and the first instance's window opens.

## D36 - Release profile
`opt-level = "s"`, thin LTO, one codegen unit, stripped: the program shrank from 18 MB to 13.7 MB. Memory did not change measurably, because it is dominated by the runtime, not by code size. `panic = "abort"` was not used so a panic in the tracker thread does not take the whole app down.

## D37 - "Prices usually change on ..." reminder
A tariff may carry `typical_update` ("MM-DD"; Bulgaria 07-01). After that date passes, a version that started before it and was not re-verified since gets an "update likely due" warning (dashboard banner, tariff editor). Re-verifying after the date, or adding a version that starts after it, clears the reminder; it returns the next year. This is the "warn when the tariff is old" requirement made specific.

## D38 - What "memory" means
Reported as three numbers because they differ a lot: private working set (what Task Manager shows by default; about 25 MB for the tray-only tracker), private bytes (about 26 MB), and total working set (about 44 MB fresh, up to about 64 MB after the window has been opened and closed, because shared library pages stay resident until Windows trims them). The 50 MB goal is met by the first two; the third only when freshly started.
## D39 - Stopwatch
A measurement of an arbitrary interval. Exact to the second: stored spans are clipped to the interval and priced with the tariff versions that applied (not read from whole-minute rows). The minute still in progress is not stored yet, so the tail after the last stored span uses the tracker's current reading; the result states how many seconds were estimated that way and how many had no data at all (never invented). Battery time keeps its kWh and costs nothing, like everywhere else. The running state and the last 50 results live in the database settings, so a measurement survives closing the window and restarting the app. Small amounts are shown in Wh and with sub-cent precision, because a few minutes of use is a fraction of a cent.

## D40 - Data folder lookup ignores empty variables
An empty `APPDATA` made the data folder a relative path in the current directory. Empty values are now treated as unset. Found when a test launch ran with a cleared variable.
## D41 - Browsing earlier periods
The dashboard takes an `offset` (0 = the current period, -1 = the one before, ...); the same command serves every period kind. Offsets are whole periods in LOCAL time: yesterday, last Monday-to-Sunday week, last calendar month, last year. A month is shifted from the first of the month so "one month back from 31 March" cannot overflow. Earlier periods are complete (no "upcoming" buckets), have no live "now" reading, and are not re-fetched every 30 seconds. Positive offsets (the future) are treated as 0. `has_earlier` (any data, minute rows, compacted hours or spans, before the period) tells the UI when to disable the back button. Compacted days are served from the aggregates and give the same figures as before compaction (tested). Switching between Today/Week/Month/Year always returns to the current period.
## D42 - Updating: a local one-click script, not an online updater
An in-app updater would need a server to host releases and signing keys, and would make PowerCost go online, which the privacy rule forbids unless opt-in and documented. The project is built from this folder, so "update" means "rebuild and reinstall": `Update PowerCost.cmd` / `scripts/update.ps1` quits the running copy, builds, installs over the existing install and restarts. It never force-kills.
* **Graceful quit.** A second launch with `--quit` is delivered to the running instance by the single-instance plugin and makes it exit through the normal path (the tracker flushes the minute in progress and writes a final heartbeat). With no instance running, `--quit` exits immediately and uses a throwaway data folder, so it cannot touch real data. An older build that does not know `--quit` just opens its window; the script then asks the user to use the tray menu and waits.
* **Explicit install folder.** The silent installer is always called with `/D=<folder>` (default `%LOCALAPPDATA%\PowerCost`) because the installer otherwise reuses whatever folder an earlier install registered, including a stale test location.
* **Build label.** `build.rs` embeds the latest commit subject and short hash (and `+edits` when the working tree is dirty), shown in Settings, so an update is verifiable (the version number stays 0.1.0).
* The data in `%APPDATA%\PowerCost` is never touched by the installer or the script.
## D45 - Faster, quieter local update
The folder-mode update (D42) was slow and showed a PowerShell console. Measured cause: the final `powercost` compile under the release profile (`lto = "thin"`, `codegen-units = 1`), plus the app being closed for the whole build. Changes in `scripts/update.ps1`:
* **Build while running, close only to install.** Order is build, quit, install, start. Tracking pauses for seconds, not minutes.
* **Quicker profile for the developer's own copy, via environment variables only:** `CARGO_PROFILE_RELEASE_LTO=false`, `CODEGEN_UNITS=16`, `INCREMENTAL=true`, and its own `CARGO_TARGET_DIR=target\update`. The published installer (`scripts/release.ps1`) still uses the Cargo.toml profile and `target\`, and the two caches no longer invalidate each other. Trade-off: the locally updated binary is a little larger and slightly slower than a release one. `-Optimized` gives the exact release build.
* **A progress window instead of a console** (`scripts/update-ui.ps1`, Windows Forms, nothing to install): four steps, a bar driven by cargo's own "N of M" counter (`CARGO_TERM_PROGRESS_WHEN=always`), "Show details" for the log, an error state with "Open log", dark/light to match Windows. The command files launch it hidden (`-WindowStyle Hidden`, no console); the app starts it with `CREATE_NO_WINDOW`. `-NoGui` keeps a console mode for automation.
* The script only asks a copy at the install path to quit, so it cannot stop a different copy by accident.
## D44 - Updates for installed copies: GitHub Releases, opt-in, verified
D42/D43 only work where the project folder exists. Everyone else installs from a release, so the app has two modes (`updater::mode()`): **folder** (the source folder embedded at build time still has `scripts/update.ps1`) and **github** (it does not). In GitHub mode the app reads `releases/latest` of the project, compares semver with its own version, finds the `*_x64-setup.exe` asset, takes its checksum from GitHub's asset `digest` (or from the release notes) and refuses a release it cannot verify. "Update now" downloads to `%TEMP%\PowerCost-update`, checks the SHA-256 and size (60 MB cap), writes a small PowerShell helper and exits normally; the helper waits for the process to end, runs the installer silently with an explicit `/D=<current install folder>` (as in D42) and starts the app again, whatever the installer's result.
* **Privacy.** This is a network call, so: off unless the user presses "Check for updates"/"Update now", or turns on the automatic daily check (default off, a new setting `online_update_check`). Requests are made by the Rust side, not the web page, so the CSP is unchanged. Only a `PowerCost/<version>` user agent is sent. Documented in PRIVACY.md before it existed in a release.
* **Allow-list.** The download URL must start with this project's `releases/download/` address and the file name must end in `_x64-setup.exe`; otherwise the release is reported as not installable.
* **Honest limit.** The installer is not code-signed. A checksum published next to the file protects against corruption, not against someone who controls the release. Signing is the real fix and costs money.
* **Test hook.** `POWERCOST_UPDATE_TEST_API` points the app at a local "latest release" document and relaxes the URL allow-list so the whole flow can be tested without publishing anything. It only matters to someone who can already set the app's environment.
* **Publishing** is `scripts/release.ps1`: it checks the version in both manifests, builds, and attaches `PowerCost_<v>_x64-setup.exe` plus a fixed-name `PowerCost-setup.exe` (for the website link). The first line of the notes is the summary shown in the app.
* The versions of internal crates no longer repeat `version = "0.1.0"` on every path dependency, so a release is one edit in the workspace manifest.
## D43 - Update button and "update available" notice, still local
The user wanted one click and a notification. The app stays offline, so "is there an update" means "does the project folder have a newer commit than this build". `build.rs` embeds the full commit hash and the absolute project folder; a background thread (first check after 60 s, then every 6 h, in 5-second slices so it stops with the app) runs `git log -1` there, compares, caches the result and shows ONE Windows notification per new commit (remembered in `update_notified.txt`). The UI shows a banner on every page and an Updates card in Settings with "Check for updates" and "Update now". "Update now" asks once ("PowerCost will close, rebuild and start again") and then starts `scripts/update.ps1` in its own console window so progress is visible; the script itself quits the app after the build via `--quit` (D42), so tracking is not interrupted during the slow compile.
* **Limits.** Uncommitted edits are not detected (only commits). If the project folder was moved or deleted the card says updates are unavailable. Not offered in `--sim` mode. Nothing is downloaded or sent anywhere.
* **Safety.** The command runs only the script path baked in at build time, never a path from the UI.