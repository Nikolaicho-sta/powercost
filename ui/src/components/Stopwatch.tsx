import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorLines, type Measurement, type StopwatchSession, type StopwatchStatus } from "../api";
import { clock, duration, energy, money, moneyPrecise, percent, STATE_LABEL } from "../format";
import { STATE_VAR } from "./Breakdown";
import { InfoTip } from "./InfoTip";
import { Notice } from "./ui";

function Tile({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="tile">
      <div className="hero-label">{label}</div>
      <div className="v">{value}</div>
      {sub && <div className="note">{sub}</div>}
    </div>
  );
}

/** Under one cent the rounded figure would read 0.00, so show the precise amount. */
function costText(m: Measurement): string {
  return m.cost_nano < 10_000_000 && m.cost_nano > 0 ? `≈ ${moneyPrecise(m.cost_nano, m.currency)}` : `≈ ${money(m.cost_minor, m.currency, m.minor_digits)}`;
}

function ResultView({ m }: { m: Measurement }) {
  const parts = [
    { k: "Measured sensors", p: m.measured_sensors_permille },
    { k: "Calibrated model", p: m.calibrated_permille },
    { k: "Default model", p: m.default_model_permille },
  ].filter((x) => x.p > 0);
  return (
    <>
      <div className="tiles">
        <Tile label="Energy used" value={energy(m.kwh_nano)} sub={`in ${duration(m.seconds)}`} />
        <Tile label="Estimated cost" value={costText(m)} sub={m.tariff_name} />
        <Tile label="Average power" value={`${m.avg_watts} W`} sub="wall power, estimated" />
      </div>

      <div className="grid" style={{ marginTop: 12 }}>
        <div className="span-6">
          <h3 className="sub">By power state</h3>
          <table className="rows">
            <thead><tr><th /><th className="n">Time</th><th className="n">Energy</th><th className="n">Cost</th></tr></thead>
            <tbody>
              {m.by_state.map((r) => (
                <tr key={r.state}>
                  <td className="name"><span className="sw" style={{ background: STATE_VAR[r.state] }} />{STATE_LABEL[r.state]}</td>
                  <td className="n">{duration(r.seconds)}</td>
                  <td className="n">{energy(r.kwh_nano)}</td>
                  <td className="n">{moneyPrecise(r.cost_nano, m.currency)}</td>
                </tr>
              ))}
              {m.by_state.length === 0 && <tr><td className="muted" colSpan={4}>No data was recorded in this time.</td></tr>}
            </tbody>
          </table>
        </div>
        <div className="span-6">
          <h3 className="sub">By tariff period</h3>
          <table className="rows">
            <thead><tr><th /><th className="n">Time</th><th className="n">Energy</th><th className="n">Cost</th></tr></thead>
            <tbody>
              {m.by_period.map((r) => (
                <tr key={r.id}>
                  <td>{r.label}</td>
                  <td className="n">{duration(r.seconds)}</td>
                  <td className="n">{energy(r.kwh_nano)}</td>
                  <td className="n">{moneyPrecise(r.cost_nano, m.currency)}</td>
                </tr>
              ))}
              {m.by_period.length === 0 && <tr><td className="muted" colSpan={4}>-</td></tr>}
            </tbody>
          </table>
        </div>
      </div>

      {parts.length > 0 && (
        <p className="note">How the watts were obtained: {parts.map((x) => `${x.k} ${percent(x.p)}`).join(", ")}.</p>
      )}
      {m.estimated_tail_seconds > 0 && <p className="note">The last {duration(m.estimated_tail_seconds)} use the current live reading because that minute was not stored yet.</p>}
      {m.untracked_seconds > 0 && <Notice kind="info">{duration(m.untracked_seconds)} of this time had no data (PowerCost was not running or the PC was off), so it is not counted.</Notice>}
    </>
  );
}

export function StopwatchPage() {
  const [status, setStatus] = useState<StopwatchStatus | null>(null);
  const [history, setHistory] = useState<StopwatchSession[]>([]);
  const [label, setLabel] = useState("");
  const [result, setResult] = useState<StopwatchSession | null>(null);
  const [errors, setErrors] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [tick, setTick] = useState(Date.now());
  const offset = useRef(0); // server clock minus this window's clock

  const refresh = useCallback(async () => {
    try {
      const [s, h] = await Promise.all([api.stopwatchStatus(), api.stopwatchHistory()]);
      offset.current = Date.parse(s.now) - Date.now();
      setStatus(s);
      setHistory(h);
    } catch (e) {
      setErrors(errorLines(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    const t = setInterval(() => setTick(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const running = status?.running ?? null;
  useEffect(() => {
    if (!running) return;
    const t = setInterval(refresh, 5000);
    return () => clearInterval(t);
  }, [running, refresh]);

  async function act(fn: () => Promise<void>) {
    setBusy(true);
    setErrors([]);
    try {
      await fn();
      await refresh();
    } catch (e) {
      setErrors(errorLines(e));
    } finally {
      setBusy(false);
    }
  }

  const elapsed = running ? (tick + offset.current - Date.parse(running.start)) / 1000 : 0;
  const live = status?.live ?? null;

  return (
    <div className="grid">
      <section className="card span-12" aria-labelledby="sw-title">
        <h2 id="sw-title">
          Stopwatch <span className="muted">· Хронометър</span>
          <InfoTip text="Press Start, use your PC, press Stop: PowerCost tells you the energy and estimated cost of exactly that time. It keeps counting if you close the window, and even across a restart of the app." />
        </h2>

        <div className="sw-face" aria-live="off">{clock(elapsed)}</div>

        {!running ? (
          <div className="controls">
            <input aria-label="What are you measuring? (optional)" placeholder="What are you measuring? (optional)" value={label} maxLength={80} style={{ width: 280 }} onChange={(e) => setLabel(e.target.value)} />
            <button type="button" className="btn primary" disabled={busy} onClick={() => act(async () => { setResult(null); await api.stopwatchStart(label); setLabel(""); })}>▶ Start</button>
          </div>
        ) : (
          <>
            <p className="hero-sub">{running.label ? <b>{running.label}</b> : "Measuring"} · started {new Date(running.start).toLocaleTimeString()}</p>
            {live && (
              <div className="tiles">
                <Tile label="Energy so far" value={energy(live.kwh_nano)} />
                <Tile label="Cost so far" value={costText(live)} sub={live.tariff_name} />
                <Tile label="Average power" value={`${live.avg_watts} W`} sub="refreshes every 5 seconds" />
              </div>
            )}
            <div className="controls">
              <button type="button" className="btn primary" disabled={busy} onClick={() => act(async () => setResult(await api.stopwatchStop()))}>■ Stop and show usage</button>
              <button type="button" className="btn" disabled={busy} onClick={() => act(() => api.stopwatchCancel())}>Discard</button>
            </div>
          </>
        )}
        {errors.length > 0 && <Notice kind="error"><ul>{errors.map((m) => <li key={m}>{m}</li>)}</ul></Notice>}
        <p className="note">All figures are estimates of your PC's wall power priced with your active tariff.</p>
      </section>

      {result && (
        <section className="card span-12" aria-labelledby="sw-result">
          <h2 id="sw-result">Result{result.label ? `: ${result.label}` : ""}</h2>
          <ResultView m={result.result} />
          <div className="controls"><button type="button" className="btn" onClick={() => setResult(null)}>Close result</button></div>
        </section>
      )}

      <section className="card span-12" aria-labelledby="sw-history">
        <h2 id="sw-history">Earlier measurements</h2>
        {history.length === 0 ? (
          <p className="muted">Nothing yet. Your finished stopwatches appear here.</p>
        ) : (
          <table className="rows">
            <thead><tr><th>When</th><th>What</th><th className="n">Time</th><th className="n">Energy</th><th className="n">Cost</th><th /></tr></thead>
            <tbody>
              {history.map((s) => (
                <tr key={s.id}>
                  <td>{new Date(s.result.start).toLocaleString()}</td>
                  <td>{s.label || <span className="muted">-</span>}</td>
                  <td className="n">{duration(s.result.seconds)}</td>
                  <td className="n">{energy(s.result.kwh_nano)}</td>
                  <td className="n">{costText(s.result)}</td>
                  <td className="n">
                    <button type="button" className="link" onClick={() => setResult(s)}>Details</button>{" "}
                    <button type="button" className="link" onClick={() => act(() => api.stopwatchDelete(s.id))} aria-label={`Delete ${s.label || "measurement"}`}>Delete</button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}
