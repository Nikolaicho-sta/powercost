import { useEffect, useMemo, useRef, useState } from "react";
import type { Dashboard, PowerStateName } from "../api";
import { bucketLabel, bucketTitle, hours, kwh, moneyPrecise, niceTicks, STATE_LABEL } from "../format";
import { STATE_VAR } from "./Breakdown";
import { InfoTip } from "./InfoTip";
import { Segmented } from "./Segmented";

const ORDER: PowerStateName[] = ["active", "idle", "sleep", "hibernate", "off"];
const H = 240;
const M = { l: 48, r: 8, t: 10, b: 26 };

type Metric = "cost" | "kwh";

function value(a: { cost_nano: number; kwh_nano: number }, m: Metric) {
  return m === "cost" ? a.cost_nano : a.kwh_nano;
}

/** Path for a bar segment with 4px rounded top corners (square at the baseline side). */
function seg(x: number, y: number, w: number, h: number, roundTop: boolean) {
  const r = roundTop ? Math.min(4, h, w / 2) : 0;
  return `M${x},${y + h} V${y + r} ${r ? `Q${x},${y} ${x + r},${y}` : ""} H${x + w - r} ${r ? `Q${x + w},${y} ${x + w},${y + r}` : ""} V${y + h} Z`;
}

export function Timeline({ d }: { d: Dashboard }) {
  const [metric, setMetric] = useState<Metric>("cost");
  const [view, setView] = useState<"chart" | "table">("chart");
  const [hover, setHover] = useState<number | null>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const [W, setW] = useState(640);

  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(Math.max(280, el.clientWidth)));
    ro.observe(el);
    setW(Math.max(280, el.clientWidth));
    return () => ro.disconnect();
  }, [view]);

  const present = useMemo(() => ORDER.filter((s) => d.timeline.some((p) => p.by_state.some((r) => r.state === s && value(r.amount, metric) > 0))), [d, metric]);
  const totals = d.timeline.map((p) => value(p.total, metric));
  const ticks = niceTicks(Math.max(0, ...totals));
  const top = ticks[ticks.length - 1];
  const plotW = W - M.l - M.r;
  const plotH = H - M.t - M.b;
  const n = d.timeline.length;
  const band = plotW / Math.max(1, n);
  const barW = Math.min(24, band * 0.72);
  const labelEvery = Math.max(1, Math.ceil(36 / band));
  // enough decimals that neighbouring ticks never print the same label
  const step = ticks.length > 1 ? ticks[1] - ticks[0] : 1;
  const tickDigits = Math.min(6, Math.max(d.minor_digits, Math.ceil(-Math.log10(step / 1e9))));
  const fmtTick = (v: number) =>
    metric === "cost"
      ? new Intl.NumberFormat(undefined, { style: "currency", currency: d.currency, minimumFractionDigits: tickDigits, maximumFractionDigits: tickDigits }).format(v / 1e9)
      : `${Number((v / 1e9).toPrecision(3))}`;
  const yOf = (v: number) => M.t + plotH - (v / top) * plotH;

  const hp = hover !== null ? d.timeline[hover] : null;

  return (
    <section className="card span-12" aria-labelledby="timeline">
      <div className="chart-head">
        <h2 id="timeline" style={{ margin: 0 }}>
          {metric === "cost" ? "Cost" : "Energy"} over time
          <InfoTip text="Each bar stacks the power states for that hour, day or month. Values are estimates; the table view lists every number." />
        </h2>
        <div style={{ display: "flex", gap: 8 }}>
          <Segmented<Metric> label="Metric" value={metric} onChange={setMetric} options={[{ value: "cost", label: "Cost" }, { value: "kwh", label: "kWh" }]} />
          <Segmented<"chart" | "table"> label="View" value={view} onChange={setView} options={[{ value: "chart", label: "Chart" }, { value: "table", label: "Table" }]} />
        </div>
      </div>

      {view === "chart" ? (
        <div className="chart" ref={wrap} onMouseLeave={() => setHover(null)}>
          <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={`Stacked ${metric === "cost" ? "cost" : "energy"} per ${d.bucket} by power state`}>
            {ticks.map((t) => (
              <g key={t}>
                <line x1={M.l} x2={W - M.r} y1={yOf(t)} y2={yOf(t)} stroke={t === 0 ? "var(--axis)" : "var(--grid)"} strokeWidth={1} />
                <text className="tick" x={M.l - 8} y={yOf(t) + 4} textAnchor="end">
                  {fmtTick(t)}
                </text>
              </g>
            ))}
            {d.timeline.map((p, i) => {
              const cx = M.l + band * i + band / 2;
              const x = cx - barW / 2;
              const parts = ORDER.map((s) => ({ s, v: value(p.by_state.find((r) => r.state === s)?.amount ?? { cost_nano: 0, kwh_nano: 0 }, metric) })).filter((q) => q.v > 0);
              let acc = 0;
              return (
                <g key={p.start}>
                  {parts.map((q, k) => {
                    const h = (q.v / top) * plotH;
                    const y = yOf(acc + q.v);
                    acc += q.v;
                    const gap = k < parts.length - 1 ? 2 : 0; // 2px surface gap between stacked segments
                    const visible = Math.max(1, h - 0);
                    return <path key={q.s} d={seg(x, y + gap, barW, Math.max(1, visible - gap), k === parts.length - 1)} fill={STATE_VAR[q.s]} opacity={hover === null || hover === i ? 1 : 0.55} />;
                  })}
                  {p.future && <line x1={x} x2={x + barW} y1={M.t + plotH - 1} y2={M.t + plotH - 1} stroke="var(--grid)" strokeWidth={2} strokeLinecap="round" />}
                  {i % labelEvery === 0 && (
                    <text className="tick" x={cx} y={H - 8} textAnchor="middle">
                      {bucketLabel(p.local_start, d.bucket)}
                    </text>
                  )}
                  {/* hit target is the whole band, larger than the mark */}
                  <rect x={M.l + band * i} y={M.t} width={band} height={plotH} fill="transparent" onMouseEnter={() => setHover(i)} onFocus={() => setHover(i)} tabIndex={-1} />
                </g>
              );
            })}
          </svg>
          {hp && hover !== null && (
            <div className="tooltip" style={{ left: Math.min(Math.max(0, M.l + band * hover + band / 2 - 80), Math.max(0, W - 170)), top: 0 }}>
              <div className="t">
                {bucketTitle(hp.local_start, d.bucket)}
                {hp.future ? " (upcoming)" : ""}
              </div>
              {hp.by_state.length === 0 && <div className="r muted">No data</div>}
              {hp.by_state.map((r) => (
                <div className="r" key={r.state}>
                  <span>
                    <span className="sw" style={{ background: STATE_VAR[r.state] }} />
                    {STATE_LABEL[r.state]}
                  </span>
                  <b>{metric === "cost" ? moneyPrecise(r.amount.cost_nano, d.currency) : kwh(r.amount.kwh_nano)}</b>
                </div>
              ))}
              {hp.by_state.length > 1 && (
                <div className="r" style={{ borderTop: "1px solid var(--grid)", marginTop: 4, paddingTop: 4 }}>
                  <span>Total</span>
                  <b>{metric === "cost" ? moneyPrecise(hp.total.cost_nano, d.currency) : kwh(hp.total.kwh_nano)}</b>
                </div>
              )}
            </div>
          )}
        </div>
      ) : (
        <div style={{ overflowX: "auto", maxHeight: 320, overflowY: "auto" }}>
          <table className="rows">
            <thead>
              <tr>
                <th>{d.bucket === "hour" ? "Hour" : d.bucket === "day" ? "Day" : "Month"}</th>
                {present.map((s) => (
                  <th className="n" key={s}>{STATE_LABEL[s]}</th>
                ))}
                <th className="n">Total</th>
                <th className="n">Time</th>
              </tr>
            </thead>
            <tbody>
              {d.timeline.filter((p) => !p.future).map((p) => (
                <tr key={p.start}>
                  <td>{bucketTitle(p.local_start, d.bucket)}</td>
                  {present.map((s) => {
                    const a = p.by_state.find((r) => r.state === s)?.amount;
                    return <td className="n" key={s}>{a ? (metric === "cost" ? moneyPrecise(a.cost_nano, d.currency) : kwh(a.kwh_nano)) : "-"}</td>;
                  })}
                  <td className="n">{metric === "cost" ? moneyPrecise(p.total.cost_nano, d.currency) : kwh(p.total.kwh_nano)}</td>
                  <td className="n muted">{hours(p.total.seconds)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {metric === "cost" && <p className="note" style={{ margin: "8px 0 0" }}>Time on battery is not billed, so those hours can show no cost; switch to kWh to see their energy.</p>}
      <div className="legend" aria-label="Legend">
        {present.map((s) => (
          <span key={s}>
            <span className="sw" style={{ background: STATE_VAR[s] }} />
            {STATE_LABEL[s]}
          </span>
        ))}
      </div>
    </section>
  );
}
