import { useCallback, useEffect, useState } from "react";
import { api, type AppInfo, type Dashboard, type PeriodKind } from "./api";
import { PeriodBreakdown, StateBreakdown } from "./components/Breakdown";
import { Hero } from "./components/Hero";
import { InfoTip } from "./components/InfoTip";
import { Segmented } from "./components/Segmented";
import { Timeline } from "./components/Timeline";
import { WhatIfPanel } from "./components/WhatIf";

type Theme = "system" | "light" | "dark";

function useTheme(): [Theme, () => void] {
  const [theme, setTheme] = useState<Theme>(() => {
    try {
      return (localStorage.getItem("theme") as Theme) || "system";
    } catch {
      return "system";
    }
  });
  useEffect(() => {
    const root = document.documentElement;
    if (theme === "system") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", theme);
    try {
      localStorage.setItem("theme", theme);
    } catch {
      /* storage may be unavailable; theme still applies for this session */
    }
  }, [theme]);
  return [theme, () => setTheme((t) => (t === "system" ? "light" : t === "light" ? "dark" : "system"))];
}

export function App() {
  const [period, setPeriod] = useState<PeriodKind>("today");
  const [data, setData] = useState<Dashboard | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [theme, cycleTheme] = useTheme();

  const load = useCallback(() => {
    api.dashboard(period).then((d) => (setData(d), setError(null))).catch((e) => setError(String(e)));
  }, [period]);

  useEffect(() => {
    load();
    const t = setInterval(load, 30_000);
    return () => clearInterval(t);
  }, [load]);
  useEffect(() => {
    api.info().then(setInfo).catch(() => undefined);
  }, []);

  return (
    <div className="app">
      <header className="top">
        <h1>⚡ PowerCost</h1>
        <div className="top-right">
          {info?.simulated && <span className="badge" title="This is generated demo data, not your real usage">Simulated data</span>}
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
          <button type="button" className="icon-btn" onClick={cycleTheme} title="Switch light / dark / system theme">
            Theme: {theme}
          </button>
        </div>
      </header>

      {error && <div className="banner" role="alert"><strong>Could not load data.</strong> {error}</div>}
      {data?.warnings.map((w) => (
        <div className="banner" role="status" key={w.code}>
          <strong>Tariff:</strong> {w.message}
        </div>
      ))}
      {info && info.sensor_issues.length > 0 && (
        <div className="banner" role="status">
          <strong>Limited sensors:</strong> {info.sensor_issues.join(" · ")}
        </div>
      )}

      {data ? (
        <div className="grid">
          <Hero d={data} />
          <section className="card span-4" aria-labelledby="about">
            <h2 id="about">
              About these numbers
              <InfoTip text="PowerCost cannot read your wall socket. It estimates watts from sensors or a model and multiplies by your tariff. A plug-in wattmeter calibration makes it much more accurate." />
            </h2>
            <p className="hero-sub" style={{ margin: 0 }}>
              All figures are <b>estimates</b>. The accuracy bar shows how the wattage was obtained. Fixed monthly
              fees are not included.
            </p>
          </section>
          <Timeline d={data} />
          <StateBreakdown d={data} />
          <PeriodBreakdown d={data} />
          <WhatIfPanel currency={data.currency} preview={!!info?.preview} />
        </div>
      ) : (
        !error && <p className="muted">Loading...</p>
      )}
    </div>
  );
}