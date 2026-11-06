# Privacy and network use

PowerCost is local-first. This page lists everything it stores and every network call it can make.

## What is stored, and where
Everything lives in `%APPDATA%\PowerCost\` on your PC:

| Item | Where | Content |
|---|---|---|
| Database | `powercost.db` | estimated usage per minute (watts, state, cost), hourly/daily totals, heartbeats, copies of Windows power events (sleep/wake/shutdown timestamps), your settings |
| Your tariffs | `tariffs\*.json` | tariffs you created or copied; plain JSON you can read, edit, share or delete |

Nothing is uploaded. There is no account, no analytics and no crash reporting.

## What it reads
* CPU/GPU power counters, battery and AC state, input idle time: from Windows, with normal user rights.
* The Windows System log (power events), through the built-in `wevtutil.exe`, only to explain time when PowerCost was not running.

## Network calls
There is exactly **one** possible network call, and it is **off by default**:

* **Optional country lookup (first-run wizard).** If you tick "Let PowerCost look up my country from my IP address" and press "Look up my country", the app requests `https://ipapi.co/json/`. That service sees your public IP address, as any website would, and answers with your country and time zone. PowerCost keeps only a two-letter country code (validated) and your confirmation of it. If you never tick the box, no request is ever made. The region is also detected locally from your language and time zone without any network use.

The app's content-security policy allows connections to that single host and nothing else. Tariff prices are **not** downloaded: they are data files shipped with the app or entered by you (a future opt-in "update tariffs" feature would be documented here before it exists).

## Other things the app does on your PC
* **Notifications:** budget alerts are ordinary local Windows notifications.
* **Start with Windows (optional, off by default):** adds one value named `PowerCost` to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (per-user, no admin rights). Turning the option off removes it.
* **Export:** writes a CSV or JSON file only to the location you choose in the save dialog.

## Removing everything
Quit PowerCost from the tray menu, turn off "Start with Windows" in Settings, then delete `%APPDATA%\PowerCost\`.
