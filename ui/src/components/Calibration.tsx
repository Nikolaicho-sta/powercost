import { useState } from "react";
import { api, errorLines, type CalibrationPoint, type Settings } from "../api";
import { dot, isDecimalString } from "../tariffModel";
import { Field, Modal, Notice } from "./ui";

const STEPS = ["What you need", "Idle reading", "Full-load reading", "Optional mid point", "Review and apply"] as const;

/** Display-only linear interpolation, mirroring the Rust estimator, to show what the curve means. */
function interpolate(points: { u: number; w: number }[], u: number): number {
  for (let i = 0; i + 1 < points.length; i++) {
    const [a, b] = [points[i], points[i + 1]];
    if (u <= b.u) return a.w + ((b.w - a.w) * (u - a.u)) / (b.u - a.u);
  }
  return points[points.length - 1].w;
}

export function CalibrationWizard({ settings, onClose, onDone }: { settings: Settings; onClose: () => void; onDone: (s: Settings) => void }) {
  const existing = settings.estimator.calibration?.points;
  const [step, setStep] = useState(0);
  const [idle, setIdle] = useState(existing?.[0]?.watts ?? "");
  const [full, setFull] = useState(existing && existing.length >= 2 ? existing[existing.length - 1].watts : "");
  const mid0 = existing?.find((p) => p.util_permille === 500)?.watts ?? "";
  const [mid, setMid] = useState(mid0);
  const [days, setDays] = useState(30);
  const [busy, setBusy] = useState(false);
  const [errors, setErrors] = useState<string[]>([]);
  const [result, setResult] = useState<string | null>(null);

  const num = (s: string) => (isDecimalString(dot(s)) ? Number(dot(s)) : NaN);
  const idleOk = num(idle) > 0 && num(idle) < 5000;
  const fullOk = num(full) > num(idle) && num(full) < 5000;
  const midOk = mid.trim() === "" || (num(mid) > num(idle) && num(mid) < num(full));
  const canNext = [true, idleOk, idleOk && fullOk, midOk, true][step];

  const points: CalibrationPoint[] = [
    { util_permille: 0, watts: dot(idle.trim()) },
    ...(mid.trim() !== "" ? [{ util_permille: 500, watts: dot(mid.trim()) }] : []),
    { util_permille: 1000, watts: dot(full.trim()) },
  ];
  const curve = points.map((p) => ({ u: p.util_permille, w: Number(p.watts) }));

  async function apply(clear: boolean) {
    setBusy(true);
    setErrors([]);
    try {
      const r = await api.saveCalibration(clear ? null : points, days);
      setResult(clear ? "Calibration removed. Estimates use sensors or the default model again." : `Calibration saved. ${r.spans} stretches of the last ${days} days were re-estimated.${r.clipped_by_compaction ? " Older days are already compacted and keep their old estimate." : ""}`);
      onDone(await api.getSettings());
    } catch (e) {
      setErrors(errorLines(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal title="Calibrate with a wattmeter" onClose={onClose} wide>
      <ol className="steps" aria-label="Steps">
        {STEPS.map((s, i) => (
          <li key={s} className={i === step ? "on" : i < step ? "done" : ""}>{s}</li>
        ))}
      </ol>

      {step === 0 && (
        <div className="prose">
          <p>A plug-in wattmeter (the kind that sits between the wall socket and a plug, about 10-20 EUR) measures what the PC really draws. Two readings turn PowerCost's estimates from a guess into a measurement-based model.</p>
          <ul>
            <li>Plug the <b>PC's charger or power cable</b> into the wattmeter, and the wattmeter into the wall. Plug in nothing else you do not want counted.</li>
            <li><b>Laptop:</b> charge the battery to 100% first. While charging, the meter also shows battery charging power, which would inflate the reading.</li>
            <li><b>Monitor:</b> leave it out of the measurement (PowerCost can add it separately), or measure with the laptop's own screen as you normally use it.</li>
            <li>Use the steady value the meter shows after it settles, in watts (W).</li>
          </ul>
          {existing && <Notice kind="info">You already have a calibration ({existing[0].watts} W idle, {existing[existing.length - 1].watts} W full). Continuing replaces it.</Notice>}
        </div>
      )}

      {step === 1 && (
        <div className="prose">
          <p><b>Idle reading.</b> Close your programs, let the PC sit untouched for about 5 minutes (screen on, as you normally use it), then read the wattmeter.</p>
          <Field label="Idle power"><span className="inline-input"><input autoFocus inputMode="decimal" value={idle} onChange={(e) => setIdle(e.target.value)} style={{ width: 90 }} /> <span className="muted">W</span></span></Field>
          {idle !== "" && !idleOk && <Notice kind="error">Enter the watts shown, a number above 0 (for example 24.5).</Notice>}
        </div>
      )}

      {step === 2 && (
        <div className="prose">
          <p><b>Full-load reading.</b> Put the PC under heavy load for about 5 minutes: run a stress test, a game or a video export that keeps the CPU (and GPU) busy, then read the highest steady value.</p>
          <Field label="Full-load power"><span className="inline-input"><input autoFocus inputMode="decimal" value={full} onChange={(e) => setFull(e.target.value)} style={{ width: 90 }} /> <span className="muted">W</span></span></Field>
          {full !== "" && !fullOk && <Notice kind="error">Full-load power must be higher than the idle reading ({idle} W).</Notice>}
        </div>
      )}

      {step === 3 && (
        <div className="prose">
          <p><b>Optional: a mid point.</b> Power does not always rise in a straight line. If you want, run a moderate load (about half of the CPU, such as video playback plus a browser) and enter that reading. Skip it to keep the straight line between idle and full.</p>
          <Field label="Half-load power (optional)"><span className="inline-input"><input inputMode="decimal" value={mid} onChange={(e) => setMid(e.target.value)} style={{ width: 90 }} /> <span className="muted">W</span></span></Field>
          {!midOk && <Notice kind="error">The mid point must be between the idle ({idle} W) and full-load ({full} W) readings.</Notice>}
        </div>
      )}

      {step === 4 && (
        <div className="prose">
          <p>PowerCost will estimate the wall power of your PC from how busy it is:</p>
          <table className="rows" style={{ maxWidth: 360 }}>
            <thead><tr><th>Load</th><th className="n">Estimated wall power</th></tr></thead>
            <tbody>
              {[0, 250, 500, 750, 1000].map((u) => (
                <tr key={u}><td>{u / 10}%</td><td className="n">{interpolate(curve, u).toFixed(1)} W</td></tr>
              ))}
            </tbody>
          </table>
          <div className="controls">
            Re-estimate the last <input type="number" min={0} max={90} value={days} style={{ width: 64 }} onChange={(e) => setDays(Math.max(0, Math.min(90, Number(e.target.value))))} /> days with this calibration
          </div>
          <p className="note">Accuracy: this is calibrated, not exact. Real draw also depends on temperature, the screen brightness and what the PC is doing. PowerCost shows the confidence level next to every figure.</p>
        </div>
      )}

      {errors.length > 0 && <Notice kind="error"><ul>{errors.map((m) => <li key={m}>{m}</li>)}</ul></Notice>}
      {result && <Notice kind="ok">{result}</Notice>}

      <div className="modal-foot">
        {existing && !result && <button type="button" className="btn danger" disabled={busy} onClick={() => apply(true)}>Remove calibration</button>}
        <span style={{ flex: 1 }} />
        {result ? (
          <button type="button" className="btn primary" onClick={onClose}>Close</button>
        ) : (
          <>
            <button type="button" className="btn" disabled={step === 0 || busy} onClick={() => setStep(step - 1)}>Back</button>
            {step < 4 ? (
              <button type="button" className="btn primary" disabled={!canNext} onClick={() => setStep(step + 1)}>Next</button>
            ) : (
              <button type="button" className="btn primary" disabled={busy} onClick={() => apply(false)}>{busy ? "Applying..." : "Save calibration"}</button>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
