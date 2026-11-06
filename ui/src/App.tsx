import { useCallback, useEffect, useState } from "react";
import { api, type AppInfo, type Dashboard, type PeriodKind, type Settings, type Theme } from "./api";
import { BudgetMeter } from "./components/BudgetMeter";
import { PeriodBreakdown, StateBreakdown } from "./components/Breakdown";
import { CalibrationWizard } from "./components/Calibration";
import { FirstRunWizard } from "./components/FirstRun";
import { Hero } from "./components/Hero";
import { InfoTip } from "./components/InfoTip";
import { Segmented } from "./components/Segmented";
import { SettingsPage } from "./components/SettingsPage";
import { TariffEditor } from "./components/TariffEditor";
import { Timeline } from "./components/Timeline";
import { WhatIfPanel } from "./components/WhatIf";

type View = "dashboard" | "tariffs" | "settings";

function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
}

export function App() {
  const [view, setView] = useState<View>("dashboard");
  const [period, setPeriod] = useState<PeriodKind>("today");
  const [data, setData] = useState<Dashboard | null>(null);
  const [month, setMonth] = useState<Dashboard | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [calibrating, setCalibrating] = useState(false);

  const load = useCallback(() => {
    api.dashboard(period).then((d) => (setData(d), setError(null))).catch((e) => setError(String(e)));
    if (settings?.budget) api.dashboard("month").then(setMonth).catch(() => undefined);
  }, [period, settings?.budget]);

  useEffect(() => {
    if (view !== "dashboard") return;
    load();
    const t = setInterval(load, 30_000);
    return () => clearInterval(t);
  }, [load, view]);

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
          {(["dashboard", "tariffs", "settings"] as View[]).map((v) => (
            <button key={v} type="button" aria-current={view === v ? "page" : undefined} className={view === v ? "on" : ""} onClick={() => setView(v)}>
              {v === "dashboard" ? "Dashboard" : v === "tariffs" ? "Tariffs" : "Settings"}
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

      {error && <div className="banner" role="alert"><strong>Could not load data.</strong> {error}</div>}
      {info?.tariff_note && <div className="banner" role="status"><strong>Tariff:</strong> {info.tariff_note}</div>}
      {info && info.sensor_issues.length > 0 && (
        <div className="banner" role="status">
          <strong>Limited sensors:</strong> {info.sensor_issues.join(" · ")}
        </div>
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

      {view === "tariffs" && <TariffEditor onChanged={() => api.info().then(setInfo)} />}
      {view === "settings" && <SettingsPage info={info} onSaved={onSaved} onCalibrate={() => setCalibrating(true)} onOpenTariffs={() => setView("tariffs")} />}

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
