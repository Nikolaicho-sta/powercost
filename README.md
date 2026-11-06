# PowerCost

PowerCost is a small Windows program that sits in the system tray and **estimates what your PC costs in electricity**: split by what the PC was doing (active, idle, sleep, off) and by tariff period (for example day and night), using the prices of your region.

* **Local-first.** Everything stays on your computer. No account, no telemetry. The only possible network call is an optional, off-by-default country lookup; see [docs/PRIVACY.md](docs/PRIVACY.md).
* **Honest.** Every figure is an *estimate*. The app shows how each number was obtained and how confident it is. Read [Accuracy and limitations](#accuracy-and-limitations) before trusting a figure.
* **Light.** The background tracker uses well under 1 % of one CPU core and about 30 MB of private memory (see the limitations for the exact numbers).

## What it does

* Tracks time in each power state and estimates watts in each. While the PC is off or asleep the app cannot run, so it records a heartbeat and afterwards classifies the gap (sleep, hibernate, shutdown, restart, crash) from the Windows event log.
* Estimates power in layers and tells you which one was used: measured CPU/GPU sensors, your own wall-meter calibration, or a default model from your hardware.
* Prices every minute with the tariff in force at that local time: weekdays, weekends, holidays, seasons, VAT, network fees and clock changes are all handled from tariff *data*, never hard-coded.
* Dashboard (today, week, month, year): cost and kWh, breakdown by state and by tariff period, a timeline, and "what if" scenarios (shut down at night; move heavy use to the cheap period).
* **Stopwatch (Хронометър):** press Start, use the PC, press Stop, and see the energy, cost, average watts and power-state breakdown of exactly that time. It keeps running if you close the window or restart the app.
* Tray tooltip with today's cost and the current watts; monthly budget alerts as Windows notifications.
* Full tariff editor, first-run wizard, calibration wizard, CSV/JSON export, optional start with Windows, light/dark theme.

## Install

Download `PowerCost_<version>_x64-setup.exe` from the build output (see [Build](#build)) and run it. It installs **per user, without administrator rights**. The first start opens a short setup wizard (region, tariff, laptop or desktop).

* PowerCost needs the Microsoft Edge WebView2 runtime. It is built into Windows 11; on Windows 10 the installer fetches it if missing.
* Closing the window does **not** quit the program: it keeps tracking from the tray. Use the tray icon's menu, **Quit PowerCost**, to stop it.
* Starting it a second time simply brings the existing window forward.
* Uninstalling removes the program and its "Start with Windows" entry. Your data in `%APPDATA%\PowerCost\` is left alone; delete that folder to remove it.

## Use

| Where | What |
|---|---|
| Tray icon | click to open the dashboard; right-click for *Open* and *Quit* |
| Dashboard | period tabs, timeline (Cost/kWh, Chart/Table), breakdowns, what-if |
| Stopwatch | start/stop a measurement of any activity (a game, a render, a night) and read its energy and cost; earlier measurements are kept |
| Tariffs | view a preset, **Copy to edit**, correct prices and hours, save a new version when prices change (in Bulgaria usually 1 July) |
| Settings | estimation, activity thresholds, budget, start with Windows, export |
| Calibration | measure idle and full-load watts with a plug-in wattmeter (10 minutes) for much better accuracy |

Command-line switches: `--tray` start without opening the window (used by "Start with Windows"); `--sim` run on generated demo data without touching your real database.

## Tariffs are data

A tariff is a versioned JSON file validated by [schema/tariff.schema.json](schema/tariff.schema.json): currency, VAT, price components (energy, network, excise, levies; each VAT-inclusive or not), periods, seasons, weekday/weekend/holiday rules, `valid_from`, `valid_to`, `source_url`, `last_verified` and a `verification` status.

* Bundled presets: Bulgaria (Electrohold, EVN, Energo-Pro, regulated household two-zone) and two structure-only examples (Germany flat, UK Economy 7).
* **Add a country** by dropping a JSON file into `%APPDATA%\PowerCost\tariffs\` and restarting. Files that do not validate are listed in the Tariffs screen with the reason.
* A price change is a **new version** of the same tariff with a later `valid_from`; earlier usage keeps the old price.

### Bulgarian presets: what is verified

| Preset | Status |
|---|---|
| Electrohold | prices and day/night hours taken from Electrohold's official page (EWRC decision Ц-13 of 1 July 2026); the decision PDF itself was not read; levies, excise and fixed fees are not on that page and are not included |
| Energo-Pro | day/night prices are on Energo-Pro's own news page; whether network fees are included is **not confirmed**; hours copied from Electrohold |
| EVN | prices from a single search-result summary, hours copied from Electrohold: **unverified**, check against your bill |

Every preset shows its status in the app. If your bill disagrees with a preset, believe your bill and correct the tariff.

## Build

Requirements: Windows 10/11, [Rust](https://rustup.rs) (stable, MSVC toolchain), Node.js 20+.

```bash
# tests (does not need the frontend)
cargo test
cargo clippy --workspace --all-targets

# frontend
cd ui
npm install
npm test
npm run build

# the app; needs ui/dist from the step above
cd ..
cargo test --workspace            # also compiles the Tauri app
cargo build --release -p powercost

# the installer (downloads the NSIS tooling the first time)
cd crates/app
../../ui/node_modules/.bin/tauri build
```

The installer is written to `target/release/bundle/nsis/`.

Try the interface without waiting for real data: `powercost.exe --sim`, or in a browser `npm run dev` inside `ui/` (it serves data exported by `cargo run -p export-mock -- ui/public/mock`).

## Project layout

| Crate | Role |
|---|---|
| `pc-core` | shared types (power states, confidence, fixed-point units) |
| `tariff-engine` | tariff JSON, validation, period lookup (DST-safe), price composition, catalogue, region guess |
| `cost-engine` | splits usage into minutes and prices them, in integer nano-units |
| `sensors` | CPU/GPU power, load, input idle, battery behind traits (Windows implementations, no admin) |
| `state-detection` | active/idle with hysteresis |
| `gap-classifier` | explains heartbeat gaps from the Windows event log (fixtures from real logs) |
| `estimator` | layered watts estimate, calibration, Modern Standby pricing |
| `storage` | SQLite: spans, minute costs, hourly/daily compaction, recompute, re-estimate |
| `tracker` | sampling loop, heartbeat, gap recovery |
| `dashboard` | read model for the UI, what-if, exports |
| `settings`, `backend` | settings, autostart, budget alerts; tariff saving and repricing, calibration |
| `sources-api` | **interface only** for future smart-plug sources (Shelly, Tasmota, Home Assistant) |
| `simulator`, `devtools`, `probe` | demo data, preview data export, a hardware probe |
| `app` | the Tauri shell: tray, window lifecycle, commands |

The reasoning behind each major choice is in [docs/DECISIONS.md](docs/DECISIONS.md). OS-specific code is behind traits so Linux and macOS can be added later.

## Accuracy and limitations

PowerCost **cannot read your wall socket**. Every figure is an estimate; treat the cost as the right order of magnitude, not an audited bill.

**How watts are estimated (and the error to expect)**

| Layer | How | Typical error |
|---|---|---|
| Calibrated | linear interpolation of the idle/full-load watts you measured with a wattmeter | about ±5 % while your usage resembles the calibration |
| Measured sensors | CPU package power (Windows Energy Meter counters) and NVIDIA GPU power (NVML), plus a *modelled* rest of system, divided by PSU efficiency | about ±15 %; the "rest" (board, display, drives) is a guess |
| Default model | from CPU/GPU TDP, drive count and a baseline | about ±30 % |

By default a wall-meter calibration wins over sensors because it measures the whole PC; you can switch that in Settings. The dashboard shows how much of each period came from which layer.

**Things that can make numbers wrong**

* **Modern Standby ("sleep").** Many laptops never really sleep: Windows reports the machine as "sleeping" while it is running with the screen off. On the reference laptop 96-98 % of "sleep" time was actively running. PowerCost prices sleep by that active share, but what the active share really draws is a model unless you calibrate.
* **Laptops.** Only mains energy is billed; battery time keeps its kWh but costs nothing. Charging after a battery period is counted as extra draw only when the battery rate is readable.
* **Monitor** power is off by default (and counted only while you are active when on). A laptop's own screen is part of the rest-of-system estimate.
* **Sensors need support.** CPU power comes from Windows "Energy Meter" counters where the platform exposes them. GPU power works for **NVIDIA** only. Without a sensor the model or your calibration is used instead (the app says so).
* **Time the app was not running.** It is explained from the Windows event log, which on some machines keeps only about five days; PowerCost copies power events into its own database to outlive that, but a gap that nothing explains uses your setting (default: shut down). The exact moment of a power cut or crash is unknown.
* **Hibernate and crash detection** follow documented Windows event meanings but were not seen on the reference machine; their tests use synthetic events.
* **Prices.** Presets can be wrong or out of date. Excise, "obligations to society" levies and fixed monthly fees may be missing from a preset; fixed fees are excluded from totals unless you switch them on. PowerCost reminds you when a tariff is old or when its usual change date (1 July in Bulgaria) has passed.
* **Rounding.** Money is integer arithmetic; sub-cent rounding drift is negligible (about 0.3 nano-kWh per minute).
* **Detail is compacted.** Minute-level detail is kept for 90 days by default, then folded into hourly and daily totals that are exact but can no longer be repriced or re-estimated.

**Resource use (measured, release build, reference laptop)**

* CPU: about 0.01 % of one core on average (0.000 % over a 70-second idle sample).
* Memory of the tracker alone, freshly started in the tray: **about 25 MB private working set** (the figure Task Manager shows by default), 26 MB private bytes, about 44 MB total working set (which also counts shared system libraries). After you have opened and closed the dashboard the process keeps more pages until Windows trims them: up to about 64 MB total working set, still about 29 MB private. Opening the window adds the WebView2 processes (roughly 100 MB or more in total); closing it frees them.
* Installer: about 3.4 MB; installed program about 14 MB.

**Not implemented**

* Smart-plug sources (Shelly, Tasmota, Home Assistant): only the interface exists ([crates/sources-api](crates/sources-api)).
* Linux and macOS.
* Automatic tariff downloads (prices are never fetched from the internet).
* AMD and Intel GPU power sensors.

## License

The workspace manifests declare MIT. There is no separate `LICENSE` file yet.
