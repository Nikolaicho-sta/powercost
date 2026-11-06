import { useEffect, useState } from "react";
import { api, type WhatIf, type WhatIfParams } from "../api";
import { hours, kwh, moneyPrecise } from "../format";
import { InfoTip } from "./InfoTip";

const HOURS = Array.from({ length: 24 }, (_, i) => i);
const hh = (h: number) => `${String(h).padStart(2, "0")}:00`;

export function WhatIfPanel({ currency, preview }: { currency: string; preview: boolean }) {
  const [p, setP] = useState<WhatIfParams>({ night_start_hour: 23, night_end_hour: 7, off_watts: "1", shift_percent: 50 });
  const [r, setR] = useState<WhatIf | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const t = setTimeout(() => {
      api.whatIf(p, 30).then((x) => live && (setR(x), setErr(null))).catch((e) => live && setErr(String(e)));
    }, 150);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [p]);

  const money = (n: number) => moneyPrecise(n, currency);
  const set = (patch: Partial<WhatIfParams>) => setP((x) => ({ ...x, ...patch }));

  return (
    <section className="card span-12" aria-labelledby="whatif">
      <h2 id="whatif">
        What if...
        <InfoTip text="Replays your last 30 days with one habit changed, using the prices that actually applied. These are estimates built on estimates; treat them as an order of magnitude." />
      </h2>
      <div className="whatif">
        <div className="scn">
          <h3>Shut the PC down at night instead of leaving it on</h3>
          <div className="controls">
            <label>
              from
              <select value={p.night_start_hour} disabled={preview} onChange={(e) => set({ night_start_hour: Number(e.target.value) })}>
                {HOURS.map((h) => <option key={h} value={h}>{hh(h)}</option>)}
              </select>
            </label>
            <label>
              until
              <select value={p.night_end_hour} disabled={preview} onChange={(e) => set({ night_end_hour: Number(e.target.value) })}>
                {HOURS.map((h) => <option key={h} value={h}>{hh(h)}</option>)}
              </select>
            </label>
            <label>
              off draw
              <input type="number" min={0} step={0.5} value={p.off_watts} disabled={preview} onChange={(e) => set({ off_watts: e.target.value || "0" })} /> W
            </label>
          </div>
          {r && (
            <>
              <div className="big">≈ {money(r.shutdown_at_night.saving_nano)} <span className="muted" style={{ fontSize: 14, fontWeight: 400 }}>saved in 30 days</span></div>
              <div className="note">
                ≈ {money(r.shutdown_at_night.saving_per_year_nano)} a year · {kwh(r.shutdown_at_night.kwh_saved_nano)} · {hours(r.shutdown_at_night.hours_affected * 3600)} of idle or sleep in that window
              </div>
            </>
          )}
        </div>

        <div className="scn">
          <h3>Move heavy use to the cheap tariff period{r ? ` (${r.cheapest_period})` : ""}</h3>
          <div className="controls">
            <label>
              share moved
              <input type="range" min={0} max={100} step={25} value={p.shift_percent} onChange={(e) => set({ shift_percent: Number(e.target.value) })} />
              {p.shift_percent}%
            </label>
          </div>
          {r && (
            <>
              <div className="big">≈ {money(r.shift_to_cheap_period.saving_nano)} <span className="muted" style={{ fontSize: 14, fontWeight: 400 }}>saved in 30 days</span></div>
              <div className="note">
                ≈ {money(r.shift_to_cheap_period.saving_per_year_nano)} a year · same energy, cheaper price · applies to {hours(r.shift_to_cheap_period.hours_affected * 3600)} of active use
              </div>
            </>
          )}
        </div>
      </div>
      {preview && <p className="note">Browser preview: night window and off draw are fixed to the defaults. They work in the app.</p>}
      {r?.note && <p className="note">{r.note}</p>}
      {err && <p className="note err">{err}</p>}
    </section>
  );
}
