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
