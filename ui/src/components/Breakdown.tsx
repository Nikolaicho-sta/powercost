import type { Dashboard, PowerStateName } from "../api";
import { hours, kwh, money, percent, share, STATE_HELP, STATE_LABEL } from "../format";
import { InfoTip } from "./InfoTip";

export const STATE_VAR: Record<PowerStateName, string> = {
  active: "var(--s-active)",
  idle: "var(--s-idle)",
  sleep: "var(--s-sleep)",
  hibernate: "var(--s-hibernate)",
  off: "var(--s-off)",
};

const PERIOD_VARS = ["var(--p-0)", "var(--p-1)", "var(--p-2)"];

interface Row { key: string; label: string; color: string; amount: Dashboard["total"]; help?: string }

function Table({ rows, total, d }: { rows: Row[]; total: Dashboard["total"]; d: Dashboard }) {
  return (
    <>
      {/* thin segmented figure: widths are the cost shares */}
      <div className="segbar" aria-hidden="true">
        {rows.filter((r) => r.amount.cost_nano > 0).map((r) => (
          <span key={r.key} style={{ background: r.color, flex: r.amount.cost_nano }} />
        ))}
      </div>
      <table className="rows">
        <thead>
          <tr>
            <th />
            <th className="n">Cost</th>
            <th className="n">Energy</th>
            <th className="n">Time</th>
            <th className="n">Share</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key}>
              <td className="name">
                <span className="sw" style={{ background: r.color }} />
                {r.label}
                {r.help && (
                  <span style={{ marginLeft: 6 }}>
                    <InfoTip text={r.help} />
                  </span>
                )}
              </td>
              <td className="n">{money(r.amount.cost_minor, d.currency, d.minor_digits)}</td>
              <td className="n">{kwh(r.amount.kwh_nano)}</td>
              <td className="n">{hours(r.amount.seconds)}</td>
              <td className="n muted">{percent(share(r.amount, total))}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

export function StateBreakdown({ d }: { d: Dashboard }) {
  const rows: Row[] = d.by_state.map((r) => ({ key: r.state, label: STATE_LABEL[r.state], color: STATE_VAR[r.state], amount: r.amount, help: STATE_HELP[r.state] }));
  return (
    <section className="card span-6" aria-labelledby="by-state">
      <h2 id="by-state">
        By power state
        <InfoTip text="Time and energy are split by what the PC was doing. Watts per state are estimates: measured CPU/GPU power where available, otherwise your wall-meter calibration or a default model." />
      </h2>
      {rows.length ? <Table rows={rows} total={d.total} d={d} /> : <p className="muted">No data in this period yet.</p>}
    </section>
  );
}

export function PeriodBreakdown({ d }: { d: Dashboard }) {
  const rows: Row[] = d.by_period.map((r, i) => ({ key: r.id, label: r.label, color: PERIOD_VARS[i] ?? "var(--ink-3)", amount: r.amount }));
  return (
    <section className="card span-6" aria-labelledby="by-period">
      <h2 id="by-period">
        By tariff period
        <InfoTip text={`Each minute is priced by the tariff period in force at that local time (${d.tariff_name}). Boundaries, weekends, seasons and clock changes are handled by the tariff data, not hard-coded.`} />
      </h2>
      <Table rows={rows} total={d.total} d={d} />
    </section>
  );
}
