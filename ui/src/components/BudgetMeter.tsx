import { money } from "../format";

/** Monthly budget meter: the fill carries severity (blue, then amber, then red) and always comes with a text label. */
export function BudgetMeter({ spentMinor, budgetMinor, currency, digits }: { spentMinor: number; budgetMinor: number; currency: string; digits: number }) {
  const pct = budgetMinor > 0 ? Math.floor((spentMinor * 100) / budgetMinor) : 0;
  const level = pct >= 100 ? "over" : pct >= 80 ? "close" : "ok";
  const label = level === "over" ? "▲ Over budget" : level === "close" ? "● Close to budget" : "✓ Within budget";
  return (
    <div className="meter-wrap">
      <div className="hero-label">Monthly budget</div>
      <div className="meter" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.min(pct, 100)} aria-label={`Monthly budget used: ${pct}%`}>
        <span className={`fill ${level}`} style={{ width: `${Math.min(100, pct)}%` }} />
      </div>
      <div className="note">
        {label}: {money(spentMinor, currency, digits)} of {money(budgetMinor, currency, digits)} ({pct}%)
      </div>
    </div>
  );
}
