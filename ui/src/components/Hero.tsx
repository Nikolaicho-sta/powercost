import type { Dashboard, PeriodKind } from "../api";
import { hours, kwh, money, percent } from "../format";
import { InfoTip } from "./InfoTip";

const LABEL: Record<PeriodKind, string> = { today: "Today so far", week: "This week", month: "This month", year: "This year" };

const LAYER_TEXT: Record<string, string> = {
  sensors: "measured CPU/GPU power + modelled rest",
  calibrated: "your wall-meter calibration",
  default: "default hardware model",
};

export function Hero({ d }: { d: Dashboard }) {
  const c = d.confidence;
  const parts = [
    { key: "measured", label: "Measured sensors", p: c.measured_sensors_permille, color: "var(--c-measured)" },
    { key: "calibrated", label: "Calibrated model", p: c.calibrated_permille, color: "var(--c-calibrated)" },
    { key: "default", label: "Default model", p: c.default_model_permille, color: "var(--c-default)" },
  ];
  const known = parts.some((p) => p.p > 0);
  const cur = d.current;
  return (
    <section className="card span-8" aria-labelledby="hero">
      <div className="hero-label" id="hero">
        Estimated cost - {LABEL[d.period]}
        <InfoTip text="Every figure is an estimate. Energy = estimated wall watts x time, priced per minute with the active tariff. The bar below shows how that wattage was obtained." />
      </div>
      <div className="hero-value">≈ {money(d.total.cost_minor, d.currency, d.minor_digits)}</div>
      <div className="hero-sub">
        {kwh(d.total.kwh_nano)} · {hours(d.total.seconds)} tracked · {d.tariff_name}
      </div>
      {known && (
        <div style={{ marginTop: 14 }}>
          <div className="segbar" role="img" aria-label="Estimate confidence mix">
            {parts.filter((p) => p.p > 0).map((p) => (
              <span key={p.key} style={{ background: p.color, flex: p.p }} />
            ))}
          </div>
          <div className="legend" style={{ marginTop: 0 }}>
            {parts.filter((p) => p.p > 0).map((p) => (
              <span key={p.key}>
                <span className="sw" style={{ background: p.color }} />
                {p.label} {percent(p.p)}
              </span>
            ))}
            <InfoTip right text="How the wattage behind this total was obtained. 'Measured sensors' still models the rest of the system (board, display, drives); only a wall meter measures everything." />
          </div>
        </div>
      )}

      {cur && (
        <div className="now">
          <span>
            Now: <b>{cur.state}</b>
          </span>
          <span>
            <b>{Number(cur.watts).toFixed(0)} W</b> <span className="muted">(about {Number(cur.low).toFixed(0)}-{Number(cur.high).toFixed(0)} W)</span>
          </span>
          <span>{cur.on_ac === false ? "on battery (not billed)" : cur.on_ac ? "on mains" : ""}</span>
          <InfoTip text={`${cur.method}. Source: ${LAYER_TEXT[cur.layer] ?? cur.layer}.`} />
        </div>
      )}
    </section>
  );
}
