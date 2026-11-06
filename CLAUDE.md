# PowerCost – electricity cost tracker for PCs

## Purpose
Desktop app that estimates how much money a PC costs in electricity, split by
power state (active / idle / sleep / off) and by tariff period (e.g. day/night),
using the electricity prices of the user's region.

## Principles
- Local-first: all data stays on the machine. No telemetry. Any network call
  (IP geolocation, tariff updates) must be opt-in and documented.
- Honest numbers: every figure is an ESTIMATE. The UI shows the estimation
  method and a confidence level (measured sensors / calibrated model / default model).
- Tariffs are DATA, never hardcoded. Prices change; they live in versioned
  JSON files with a `valid_from` date and a `source_url`.
- Low overhead: the background tracker must use <1% CPU and <50 MB RAM on average.
- Windows 10/11 first. Keep OS-specific code behind interfaces so Linux/macOS
  can be added later.
- Money math uses integer minor units or Decimal types, never floats.
- Time: store everything in UTC, convert to local time only for tariff lookup
  and display. Handle DST changes correctly.

## Conventions
- Write tests for all cost/tariff/gap-detection logic before the UI.
- Small commits, conventional commit messages.
- Keep a docs/DECISIONS.md with the reasoning behind each major choice.