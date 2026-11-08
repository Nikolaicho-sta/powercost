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
There are exactly **two** possible network uses. The update notification (first one) is **on by default**; you can switch it off in the setup wizard or in Settings. The country lookup never happens unless you ask for it:

* **Update check and download (GitHub Releases).** Only for copies installed from a release, i.e. without the project folder (see D43/D44). It talks to `github.com` / `api.github.com` (the project's releases page) when you press "Check for updates" or "Update now", and about once a day in the background to show a "new version available" notification, unless you untick "Tell me when a new version is available" in the first-run setup or "Look for updates automatically" in Settings (ticked/on by default, a decision recorded in D47; the setup question is shown only for copies that update from GitHub). The background check only notifies; it never downloads or installs anything. The request is an ordinary web request for the newest release's description; GitHub sees your IP address and a `PowerCost/<version>` user agent, as any website would. Nothing about you or your usage is sent. When you press "Update now" the installer is downloaded from the same release, checked against its published SHA-256, and only then run. The installer is not code-signed, so the checksum protects against a corrupted download but not against a compromised GitHub account. The code is in `crates/app/src/online.rs`.

* **Optional country lookup (first-run wizard).** If you tick "Let PowerCost look up my country from my IP address" and press "Look up my country", the app requests `https://ipapi.co/json/`. That service sees your public IP address, as any website would, and answers with your country and time zone. PowerCost keeps only a two-letter country code (validated) and your confirmation of it. If you never tick the box, no request is ever made. The region is also detected locally from your language and time zone without any network use.

The window's content-security policy lets the interface itself connect to the single host `ipapi.co` and nothing else; the GitHub update requests are made by the program, never by the web page. Tariff prices are **not** downloaded: they are data files shipped with the app or entered by you (a future opt-in "update tariffs" feature would be documented here before it exists).

## Other things the app does on your PC
* **Notifications:** budget alerts and "update available" notices are ordinary local Windows notifications.
* **Update check (copy built from a project folder, i.e. the developer's):** about a minute after start and then every 6 hours, the app runs `git log -1` in the project folder it was built from and compares the commit with its own. No network is involved; the result is only used to show a notice and the "Update now" button. A marker file `update_notified.txt` in the data folder remembers which commit you were already told about. "Update now" runs `scripts\update.ps1` from that folder (see D42, D43).
* **Start with Windows (optional, off by default):** adds one value named `PowerCost` to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (per-user, no admin rights). Turning the option off removes it.
* **Export:** writes a CSV or JSON file only to the location you choose in the save dialog.

## Removing everything
Quit PowerCost from the tray menu, turn off "Start with Windows" in Settings, then delete `%APPDATA%\PowerCost\`.
