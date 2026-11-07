import { useCallback, useEffect, useState } from "react";
import { api, type AppInfo, type Dashboard, type PeriodKind, type Settings, type Theme, type UpdateStatus } from "./api";
import { UpdateBanner } from "./components/UpdateBox";
import { PERIOD_UNIT, periodTitle } from "./format";
import { BudgetMeter } from "./components/BudgetMeter";
import { PeriodBreakdown, StateBreakdown } from "./components/Breakdown";
import { CalibrationWizard } from "./components/Calibration";
import { FirstRunWizard } from "./components/FirstRun";
import { Hero } from "./components/Hero";
import { InfoTip } from "./components/InfoTip";
import { Segmented } from "./components/Segmented";
import { SettingsPage } from "./components/SettingsPage";
import { StopwatchPage } from "./components/Stopwatch";
import { TariffEditor } from "./components/TariffEditor";
import { Timeline } from "./components/Timeline";
import { WhatIfPanel } from "./components/WhatIf";

type View = "dashboard" | "stopwatch" | "tariffs" | "settings";

function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
}

export function App() {
  const [view, setView] = useState<View>("dashboard");
  const [period, setPeriodState] = useState<PeriodKind>("today");
  const [offset, setOffset] = useState(0); // 0 = current period, -1 = the one before, ...
  const [data, setData] = useState<Dashboard | null>(null);
  const [month, setMonth] = useState<Dashboard | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [calibrating, setCalibrating] = useState(false);
  const [update, setUpdate] = useState<UpdateStatus>({ state: "unknown" });

  // Switching between Today/Week/Month/Year always returns to the current period.
  const setPeriod = (p: PeriodKind) => {
    setPeriodState(p);
    setOffset(0);
  };

  const load = useCallback(() => {
    api.dashboard(period, offset).then((d) => (setData(d), setError(null))).catch((e) => setError(String(e)));
    if (settings?.budget) api.dashboard("month").then(setMonth).catch(() => undefined);
  }, [period, offset, settings?.budget]);

  useEffect(() => {
    if (view !== "dashboard") return;
    load();
    if (offset !== 0) return; // earlier periods do not change
    const t = setInterval(load, 30_000);
    return () => clearInterval(t);
  }, [load, view, offset]);

  const canGoBack = !!data && data.has_earlier;
  const canGoForward = offset < 0;
  const goBack = useCallback(() => setOffset((o) => o - 1), []);
  const goForward = useCallback(() => setOffset((o) => Math.min(0, o + 1)), []);

  // Left/Right arrow keys step through periods (not while typing or while a dialog is open).
  useEffect(() => {
    if (view !== "dashboard") return;
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (e.altKey || e.ctrlKey || e.metaKey || (el && /^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName)) || document.querySelector(".modal-back")) return;
      if (e.key === "ArrowLeft" && canGoBack) goBack();
      if (e.key === "ArrowRight" && canGoForward) goForward();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view, canGoBack, canGoForward, goBack, goForward]);

  // The background checker refreshes the cached status every few hours; read it now and every 10 min.
  useEffect(() => {
    const read = () => api.updateStatus().then(setUpdate).catch(() => undefined);
    read();
    const t = setInterval(read, 600_000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    api.info().then(setInfo).catch(() => undefined);
    api.getSettings().then((s) => (setSettings(s), applyTheme(s.theme))).catch((e) => setError(String(e)));
  }, []);

  const onSaved = (s: Settings) => {
    setSettings(s);
    applyTheme(s.theme);
    api.info().then(setInfo).catch(() => undefined);
  };

  const cycleTheme = async () => {
    if (!settings) return;
    const next: Theme = settings.theme === "system" ? "light" : settings.theme === "light" ? "dark" : "system";
    applyTheme(next);
    try {
      onSaved(await api.saveSettings({ ...settings, theme: next }));
    } catch {
      /* the theme still changed for this session */
    }
  };

  return (
    <div className="app">
      <header className="top">
        <h1>⚡ PowerCost</h1>
        <nav className="tabs" aria-label="Sections">
          {(["dashboard", "stopwatch", "tariffs", "settings"] as View[]).map((v) => (
            <button key={v} type="button" aria-current={view === v ? "page" : undefined} className={view === v ? "on" : ""} onClick={() => setView(v)}>
              {v === "dashboard" ? "Dashboard" : v === "stopwatch" ? "Stopwatch" : v === "tariffs" ? "Tariffs" : "Settings"}
            </button>
          ))}
        </nav>
        <div className="top-right">
          {info?.simulated && <span className="badge" title="This is generated demo data, not your real usage">Simulated data</span>}
          {view === "dashboard" && (
            <Segmented<PeriodKind>
              label="Period"
              value={period}
              onChange={setPeriod}
              options={[
                { value: "today", label: "Today" },
                { value: "week", label: "Week" },
                { value: "month", label: "Month" },
                { value: "year", label: "Year" },
              ]}
            />
          )}
          <button type="button" className="icon-btn" onClick={cycleTheme} title="Switch light / dark / system theme">
            Theme: {settings?.theme ?? "system"}
          </button>
        </div>
      </header>

      <UpdateBanner status={update} />
      {error && <div className="banner" role="alert"><strong>Could not load data.</strong> {error}</div>}
      {info?.tariff_note && <div className="banner" role="status"><strong>Tariff:</strong> {info.tariff_note}</div>}
      {info && info.sensor_issues.length > 0 && (
        <div className="banner" role="status">
          <strong>Limited sensors:</strong> {info.sensor_issues.join(" · ")}
        </div>
      )}

      {view === "dashboard" && data && (
        <nav className="period-nav" aria-label={`Browse ${PERIOD_UNIT[period]}s`}>
          <button type="button" className="icon-btn" disabled={!canGoBack} onClick={goBack} aria-label={`Previous ${PERIOD_UNIT[period]}`} title={`Previous ${PERIOD_UNIT[period]} (left arrow key)`}>
            ◀
          </button>
          <div className="period-title" aria-live="polite">
            {periodTitle(data.period, data.period_start, data.period_end_inclusive)}
            {data.offset === 0 && <span className="muted"> · now</span>}
          </div>
          <button type="button" className="icon-btn" disabled={!canGoForward} onClick={goForward} aria-label={`Next ${PERIOD_UNIT[period]}`} title={`Next ${PERIOD_UNIT[period]} (right arrow key)`}>
            ▶
          </button>
          {offset !== 0 && (
            <button type="button" className="btn" onClick={() => setOffset(0)}>
              Back to {period === "today" ? "today" : `this ${PERIOD_UNIT[period]}`}
            </button>
          )}
        </nav>
      )}

      {view === "dashboard" &&
        (data ? (
          <>
            {data.warnings.map((w) => (
              <div className="banner" role="status" key={w.code}>
                <strong>Tariff:</strong> {w.message}{" "}
                <button type="button" className="link" onClick={() => setView("tariffs")}>Update tariff</button>
              </div>
            ))}
            <div className="grid">
              <Hero d={data} />
              <section className="card span-4" aria-labelledby="about">
                <h2 id="about">
                  About these numbers
                  <InfoTip text="PowerCost cannot read your wall socket. It estimates watts from sensors or a model and multiplies by your tariff. A plug-in wattmeter calibration makes it much more accurate." />
                </h2>
                <p className="hero-sub" style={{ margin: 0 }}>
                  All figures are <b>estimates</b>. The accuracy bar shows how the wattage was obtained.{" "}
                  {data.fixed_fees ? "Fixed monthly fees are included." : "Fixed monthly fees are not included."}
                </p>
                {settings?.budget && month && (
                  <div style={{ marginTop: 14 }}>
                    <BudgetMeter spentMinor={month.grand_total_minor} budgetMinor={settings.budget.monthly_minor} currency={month.currency} digits={month.minor_digits} />
                  </div>
                )}
                {settings && !settings.estimator.calibration && (
                  <p className="note" style={{ marginTop: 12 }}>
                    Want better accuracy? <button type="button" className="link" onClick={() => setCalibrating(true)}>Calibrate with a wattmeter</button>
                  </p>
                )}
              </section>
              <Timeline d={data} />
              <StateBreakdown d={data} />
              <PeriodBreakdown d={data} />
              <WhatIfPanel currency={data.currency} preview={!!info?.preview} />
            </div>
          </>
        ) : (
          !error && <p className="muted">Loading...</p>
        ))}

      {view === "stopwatch" && <StopwatchPage />}
      {view === "tariffs" && <TariffEditor onChanged={() => api.info().then(setInfo)} />}
      {view === "settings" && <SettingsPage info={info} update={update} onUpdate={setUpdate} onSaved={onSaved} onCalibrate={() => setCalibrating(true)} onOpenTariffs={() => setView("tariffs")} />}

      {calibrating && settings && (
        <CalibrationWizard settings={settings} onClose={() => setCalibrating(false)} onDone={onSaved} />
      )}
      {settings && !settings.first_run_done && (
        <FirstRunWizard
          initial={settings}
          onDone={(s, openCal, openTariffs) => {
            onSaved(s);
            if (openTariffs) setView("tariffs");
            if (openCal) setCalibrating(true);
          }}
        />
      )}
    </div>
  );
}
