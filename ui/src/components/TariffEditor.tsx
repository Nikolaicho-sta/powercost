import { useCallback, useEffect, useMemo, useState } from "react";
import { api, errorLines, type PresetInfo, type TariffList, type ValidationReport } from "../api";
import {
  addPeriod, blankTariff, copyAsUser, dot, nextVersion, overnightRanges, parseHolidays, removePeriod, renamePeriod, setComponentMode, slugify,
  type DayKind, type Rule, type Season, type TariffDoc, type Verification,
} from "../tariffModel";
import { InfoTip } from "./InfoTip";
import { Badge, Check, Field, Notice, verificationBadge } from "./ui";

const DAYS: DayKind[] = ["all", "weekday", "weekend", "saturday", "sunday", "holiday"];
const KINDS = ["energy", "network", "excise", "levy", "other"] as const;
const today = () => new Date().toISOString().slice(0, 10);

function normalise(raw: unknown): TariffDoc {
  const t = raw as TariffDoc;
  return { ...t, fixed_fees: t.fixed_fees ?? [], holidays: t.holidays ?? [], components: t.components ?? [], periods: t.periods ?? [], schedule: t.schedule ?? { seasons: [] } };
}

function Row({ children }: { children: React.ReactNode }) {
  return <div className="trow">{children}</div>;
}

export function TariffEditor({ onChanged }: { onChanged: () => void }) {
  const [list, setList] = useState<TariffList | null>(null);
  const [doc, setDoc] = useState<TariffDoc | null>(null);
  const [orig, setOrig] = useState<string>("");
  const [sel, setSel] = useState<{ id: string; version: number } | null>(null);
  const [readOnly, setReadOnly] = useState(true);
  const [isNew, setIsNew] = useState(false);
  const [report, setReport] = useState<ValidationReport | null>(null);
  const [msg, setMsg] = useState<{ kind: "ok" | "error" | "info"; text: string[] } | null>(null);
  const [holidayText, setHolidayText] = useState("");
  const [newFrom, setNewFrom] = useState(today());
  const [preview, setPreview] = useState(false);
  const [overnight, setOvernight] = useState({ start: "22:00", end: "06:00" });

  const refresh = useCallback(async () => {
    const l = await api.listTariffs();
    setList(l);
    return l;
  }, []);

  useEffect(() => {
    refresh();
    api.info().then((i) => setPreview(i.preview));
  }, [refresh]);

  // live validation (debounced); the engine in Rust is the authority
  useEffect(() => {
    if (!doc) return;
    const t = setTimeout(() => {
      api.validateTariff(doc).then(setReport).catch((e) => setReport({ ok: false, errors: errorLines(e), warnings: [], prices: [] }));
    }, 300);
    return () => clearTimeout(t);
  }, [doc]);

  const open = async (p: PresetInfo) => {
    const d = normalise(await api.getTariff(p.id, p.version));
    setDoc(d);
    setOrig(JSON.stringify(d));
    setSel({ id: p.id, version: p.version });
    setReadOnly(p.source === "bundled");
    setIsNew(false);
    setHolidayText(d.holidays.join(", "));
    setMsg(null);
    setNewFrom(today());
  };

  const startNew = async () => {
    const s = await api.getSettings();
    const tz = (() => { try { return Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { return "UTC"; } })();
    const d = blankTariff(s.region.country ?? "BG", "EUR", tz, today());
    setDoc(d);
    setOrig("");
    setSel(null);
    setReadOnly(false);
    setIsNew(true);
    setHolidayText("");
    setMsg({ kind: "info", text: ["Fill in your prices from your electricity bill. Prices are per kWh, in the tariff currency."] });
  };

  const copy = () => {
    if (!doc) return;
    const d = copyAsUser(doc, today());
    setDoc(d);
    setOrig("");
    setSel(null);
    setReadOnly(false);
    setIsNew(true);
    setMsg({ kind: "info", text: ["This is your own copy, marked unverified. Check every price against your bill, then save."] });
  };

  const upd = (f: (d: TariffDoc) => void) => {
    if (!doc || readOnly) return;
    const c = structuredClone(doc);
    f(c);
    setDoc(c);
    setMsg(null);
  };
  const replace = (d: TariffDoc) => {
    if (!readOnly) {
      setDoc(d);
      setMsg(null);
    }
  };

  const dirty = doc !== null && JSON.stringify(doc) !== orig;
  const canSave = !!report?.ok && !readOnly && (dirty || isNew);

  async function persist(toSave: TariffDoc, recomputeFrom: string | null) {
    try {
      const res = await api.saveTariff(toSave, recomputeFrom);
      const l = await refresh();
      const info = l.presets.find((p) => p.id === res.info.id && p.version === res.info.version);
      if (info) await open(info);
      onChanged();
      const lines = [`Saved "${res.info.name}" version ${res.info.version}.`];
      if (res.recomputed_spans !== null) lines.push(`Prices were re-applied to ${res.recomputed_spans} stretches of recent usage.`);
      if (res.clipped_by_compaction) lines.push("Part of the range is older than the detailed history, so those days keep their old prices.");
      setMsg({ kind: "ok", text: lines });
    } catch (e) {
      setMsg({ kind: "error", text: errorLines(e) });
    }
  }

  async function useIt() {
    if (!doc) return;
    try {
      const s = await api.getSettings();
      s.tariff_id = doc.id;
      await api.saveSettings(s);
      await refresh();
      onChanged();
      setMsg({ kind: "ok", text: [`"${doc.name}" is now the active tariff. Use "Reprice" in Settings to apply it to past usage.`] });
    } catch (e) {
      setMsg({ kind: "error", text: errorLines(e) });
    }
  }

  async function remove() {
    if (!sel || !confirm(`Delete "${doc?.name}" version ${sel.version}? This cannot be undone.`)) return;
    try {
      await api.deleteTariff(sel.id, sel.version);
      await refresh();
      setDoc(null);
      setSel(null);
      setMsg({ kind: "ok", text: ["Deleted."] });
    } catch (e) {
      setMsg({ kind: "error", text: errorLines(e) });
    }
  }

  const groups = useMemo(() => {
    const m = new Map<string, PresetInfo[]>();
    for (const p of list?.presets ?? []) m.set(p.country, [...(m.get(p.country) ?? []), p]);
    return [...m.entries()];
  }, [list]);

  const periodIds = doc?.periods.map((p) => p.id) ?? [];
  const holidays = parseHolidays(holidayText);

  return (
    <div className="grid">
      <aside className="card span-4" aria-label="Tariffs">
        <h2>Tariffs</h2>
        <div className="controls">
          <button type="button" className="btn primary" onClick={startNew}>New tariff</button>
        </div>
        {groups.map(([country, items]) => (
          <div key={country}>
            <div className="group-head">{country}</div>
            <ul className="tlist">
              {items.map((p) => (
                <li key={`${p.id}-${p.version}`}>
                  <button type="button" className={`titem${sel?.id === p.id && sel.version === p.version ? " on" : ""}`} onClick={() => open(p)}>
                    <span><b>{p.name}</b></span>
                    <span className="note">
                      v{p.version} - from {p.valid_from} {p.source === "user" ? <Badge tone="good">mine</Badge> : null} {verificationBadge(p.verification)} {list?.selected_id === p.id ? <Badge tone="good">active</Badge> : null}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ))}
        {list && list.problems.length > 0 && <Notice kind="error"><b>Some tariff files could not be loaded:</b><ul>{list.problems.map((p) => <li key={p}>{p}</li>)}</ul></Notice>}
        <p className="note">Your tariffs are plain JSON files in <code>{list?.user_dir}</code>. To add a country, drop a tariff file there and restart.</p>
      </aside>

      <section className="card span-8" aria-label="Tariff editor">
        {!doc ? (
          <p className="muted">Choose a tariff on the left to view it, or create a new one.</p>
        ) : (
          <>
            <div className="chart-head">
              <h2 style={{ margin: 0 }}>{doc.name || "(unnamed)"} {readOnly ? <Badge>read-only preset</Badge> : isNew ? <Badge tone="warn">new</Badge> : <Badge tone="good">mine</Badge>}</h2>
              <div className="controls" style={{ margin: 0 }}>
                {readOnly && <button type="button" className="btn primary" onClick={copy}>Copy to edit</button>}
                {sel && list?.selected_id !== doc.id && <button type="button" className="btn" onClick={useIt}>Use this tariff</button>}
                {sel && !readOnly && list?.selected_id !== doc.id && <button type="button" className="btn danger" onClick={remove}>Delete</button>}
              </div>
            </div>

            {readOnly && <Notice kind="info">Bundled presets cannot be changed, so a wrong preset never overwrites itself. Use <b>Copy to edit</b> to correct a price or hours.</Notice>}

            <h3 className="sub">Basics</h3>
            <div className="form two">
              <Field label="Name"><input value={doc.name} disabled={readOnly} onChange={(e) => upd((d) => { d.name = e.target.value; if (isNew && d.id.startsWith("user-")) d.id = `user-${slugify(e.target.value)}`; })} /></Field>
              <Field label="Identifier" hint="Lower-case letters, digits, dashes. Fixed after the first save."><input value={doc.id} disabled={readOnly || !isNew} onChange={(e) => upd((d) => (d.id = e.target.value))} /></Field>
              <Field label="Country"><input value={doc.country} maxLength={2} disabled={readOnly} style={{ width: 70, textTransform: "uppercase" }} onChange={(e) => upd((d) => (d.country = e.target.value.toUpperCase()))} /></Field>
              <Field label="Distributor / provider"><input value={doc.distributor ?? ""} disabled={readOnly} onChange={(e) => upd((d) => (d.distributor = e.target.value || null))} /></Field>
              <Field label="Currency"><input value={doc.currency} maxLength={3} disabled={readOnly} style={{ width: 80, textTransform: "uppercase" }} onChange={(e) => upd((d) => (d.currency = e.target.value.toUpperCase()))} /></Field>
              <Field label="Time zone" help="IANA name, for example Europe/Sofia. Schedules are read in this local time, and clock changes are handled for you."><input value={doc.timezone} disabled={readOnly} onChange={(e) => upd((d) => (d.timezone = e.target.value))} /></Field>
              <Field label="VAT %" help="Applied to components that are not marked VAT-inclusive."><input inputMode="decimal" value={doc.vat_percent} disabled={readOnly} style={{ width: 80 }} onChange={(e) => upd((d) => (d.vat_percent = dot(e.target.value)))} /></Field>
              <Field label="Version"><input value={doc.version} disabled style={{ width: 60 }} /></Field>
              <Field label="Valid from"><input type="date" value={doc.valid_from} disabled={readOnly} onChange={(e) => upd((d) => (d.valid_from = e.target.value))} /></Field>
              <Field label="Valid to (optional)"><input type="date" value={doc.valid_to ?? ""} disabled={readOnly} onChange={(e) => upd((d) => (d.valid_to = e.target.value || null))} /></Field>
              <Field label="Source link"><input value={doc.source_url ?? ""} disabled={readOnly} placeholder="https://..." onChange={(e) => upd((d) => (d.source_url = e.target.value || null))} /></Field>
              <Field label="Last checked against the source"><input type="date" value={doc.last_verified ?? ""} disabled={readOnly} onChange={(e) => upd((d) => (d.last_verified = e.target.value || null))} /></Field>
              <Field label="Prices usually change on (MM-DD)" help="For example 07-01 in Bulgaria. After that date PowerCost reminds you to check the prices if you have not re-checked them since."><input value={doc.typical_update ?? ""} placeholder="07-01" disabled={readOnly} style={{ width: 90 }} onChange={(e) => upd((d) => (d.typical_update = e.target.value.trim() || null))} /></Field>
              <Field label="Verification"><select value={doc.verification ?? "unverified"} disabled={readOnly} onChange={(e) => upd((d) => (d.verification = e.target.value as Verification))}>{["verified", "partial", "unverified", "example"].map((v) => <option key={v}>{v}</option>)}</select></Field>
            </div>
            <Field label="Notes"><textarea rows={2} value={doc.notes ?? ""} disabled={readOnly} onChange={(e) => upd((d) => (d.notes = e.target.value))} /></Field>

            <h3 className="sub">Tariff periods <InfoTip text="For example day and night, or peak and off-peak. A flat tariff has a single period." /></h3>
            {doc.periods.map((p) => (
              <Row key={p.id}>
                <input aria-label="Period id" value={p.id} disabled={readOnly} style={{ width: 120 }} onChange={(e) => replace(renamePeriod(doc, p.id, e.target.value))} />
                <input aria-label="Period label" value={p.label} disabled={readOnly} onChange={(e) => upd((d) => { const x = d.periods.find((q) => q.id === p.id); if (x) x.label = e.target.value; })} />
                {!readOnly && doc.periods.length > 1 && <button type="button" className="btn" onClick={() => replace(removePeriod(doc, p.id))}>Remove</button>}
              </Row>
            ))}
            {!readOnly && <button type="button" className="btn" onClick={() => { let n = doc.periods.length + 1; while (periodIds.includes(`period_${n}`)) n++; replace(addPeriod(doc, `period_${n}`, `Period ${n}`)); }}>Add period</button>}

            <h3 className="sub">Price components (per kWh) <InfoTip text="Add each part of the price: the energy price, network and distribution fees, excise, other levies. Tick VAT-inclusive for parts whose price already contains VAT." /></h3>
            {doc.components.map((c, ci) => (
              <div className="comp" key={ci}>
                <Row>
                  <input aria-label="Component name" value={c.label ?? c.id} disabled={readOnly} onChange={(e) => upd((d) => { d.components[ci].label = e.target.value; })} />
                  <select aria-label="Kind" value={c.kind} disabled={readOnly} onChange={(e) => upd((d) => { d.components[ci].kind = e.target.value as (typeof KINDS)[number]; })}>{KINDS.map((k) => <option key={k}>{k}</option>)}</select>
                  <select aria-label="Pricing" value={c.per_period ? "per_period" : "flat"} disabled={readOnly} onChange={(e) => upd((d) => { d.components[ci] = setComponentMode(d.components[ci], e.target.value as "flat" | "per_period", d.periods); })}>
                    <option value="flat">same in every period</option>
                    <option value="per_period">per period</option>
                  </select>
                  <Check label="VAT included" checked={c.vat_inclusive} disabled={readOnly} onChange={(v) => upd((d) => (d.components[ci].vat_inclusive = v))} />
                  {!readOnly && doc.components.length > 1 && <button type="button" className="btn" onClick={() => upd((d) => d.components.splice(ci, 1))}>Remove</button>}
                </Row>
                <Row>
                  {c.per_period ? (
                    doc.periods.map((p) => (
                      <label key={p.id} className="inline-input">{p.label}: <input inputMode="decimal" aria-label={`${c.label ?? c.id} ${p.label} price`} style={{ width: 90 }} value={c.per_period?.[p.id] ?? ""} disabled={readOnly} onChange={(e) => upd((d) => { d.components[ci].per_period![p.id] = dot(e.target.value); })} /></label>
                    ))
                  ) : (
                    <label className="inline-input">Price: <input inputMode="decimal" aria-label={`${c.label ?? c.id} price`} style={{ width: 100 }} value={c.per_kwh ?? ""} disabled={readOnly} onChange={(e) => upd((d) => { d.components[ci].per_kwh = dot(e.target.value); })} /></label>
                  )}
                  <span className="muted">{doc.currency}/kWh</span>
                </Row>
              </div>
            ))}
            {!readOnly && <button type="button" className="btn" onClick={() => upd((d) => d.components.push({ id: `part_${d.components.length + 1}`, kind: "network", label: "Network fee", vat_inclusive: false, per_kwh: "0.00" }))}>Add component</button>}

            <h3 className="sub">Fixed monthly fees <InfoTip text="Subscription or meter fees. They are left out of the per-kWh cost unless you switch on 'Include fixed monthly fees' in Settings." /></h3>
            {doc.fixed_fees.map((f, fi) => (
              <Row key={fi}>
                <input aria-label="Fee name" value={f.label ?? f.id} disabled={readOnly} onChange={(e) => upd((d) => { d.fixed_fees[fi].label = e.target.value; })} />
                <input aria-label="Fee per month" inputMode="decimal" style={{ width: 90 }} value={f.per_month} disabled={readOnly} onChange={(e) => upd((d) => { d.fixed_fees[fi].per_month = dot(e.target.value); })} /> <span className="muted">{doc.currency}/month</span>
                <Check label="VAT included" checked={f.vat_inclusive} disabled={readOnly} onChange={(v) => upd((d) => (d.fixed_fees[fi].vat_inclusive = v))} />
                {!readOnly && <button type="button" className="btn" onClick={() => upd((d) => d.fixed_fees.splice(fi, 1))}>Remove</button>}
              </Row>
            ))}
            {!readOnly && <button type="button" className="btn" onClick={() => upd((d) => d.fixed_fees.push({ id: `fee_${d.fixed_fees.length + 1}`, label: "Monthly fee", per_month: "0.00", vat_inclusive: true }))}>Add fixed fee</button>}

            <h3 className="sub">When each period applies <InfoTip text="Seasons split the year (dates are month-day). Inside a season, each rule says which days and which local time ranges belong to a period. Ranges cannot cross midnight: use the overnight helper. Every minute of every day type must be covered exactly once." /></h3>
            {doc.schedule.seasons.map((s: Season, si) => (
              <div className="season" key={si}>
                <Row>
                  <b>Season</b>
                  <input aria-label="Season name" value={s.id} disabled={readOnly} style={{ width: 110 }} onChange={(e) => upd((d) => (d.schedule.seasons[si].id = e.target.value))} />
                  <label className="inline-input">from <input aria-label="Season from" value={s.from} placeholder="MM-DD" disabled={readOnly} style={{ width: 70 }} onChange={(e) => upd((d) => (d.schedule.seasons[si].from = e.target.value))} /></label>
                  <label className="inline-input">to <input aria-label="Season to" value={s.to} placeholder="MM-DD" disabled={readOnly} style={{ width: 70 }} onChange={(e) => upd((d) => (d.schedule.seasons[si].to = e.target.value))} /></label>
                  {!readOnly && doc.schedule.seasons.length > 1 && <button type="button" className="btn" onClick={() => upd((d) => d.schedule.seasons.splice(si, 1))}>Remove season</button>}
                </Row>
                {s.rules.map((r: Rule, ri) => (
                  <div className="rule" key={ri}>
                    <div className="chips" role="group" aria-label="Days">
                      {DAYS.map((dk) => (
                        <label key={dk} className={`chip${r.days.includes(dk) ? " on" : ""}`}>
                          <input type="checkbox" checked={r.days.includes(dk)} disabled={readOnly} onChange={(e) => upd((d) => { const days = d.schedule.seasons[si].rules[ri].days; d.schedule.seasons[si].rules[ri].days = e.target.checked ? [...days, dk] : days.filter((x) => x !== dk); })} />
                          {dk}
                        </label>
                      ))}
                    </div>
                    <Row>
                      <label className="inline-input">period
                        <select aria-label="Rule period" value={r.period} disabled={readOnly} onChange={(e) => upd((d) => (d.schedule.seasons[si].rules[ri].period = e.target.value))}>{doc.periods.map((p) => <option key={p.id} value={p.id}>{p.label}</option>)}</select>
                      </label>
                      {r.ranges.map((rg, gi) => (
                        <span className="inline-input" key={gi}>
                          <input type="time" aria-label="From" value={rg.start} disabled={readOnly} onChange={(e) => upd((d) => (d.schedule.seasons[si].rules[ri].ranges[gi].start = e.target.value))} />-
                          <input aria-label="To" value={rg.end} disabled={readOnly} style={{ width: 66 }} placeholder="24:00" onChange={(e) => upd((d) => (d.schedule.seasons[si].rules[ri].ranges[gi].end = e.target.value))} />
                          {!readOnly && r.ranges.length > 1 && <button type="button" className="icon-btn" aria-label="Remove range" onClick={() => upd((d) => d.schedule.seasons[si].rules[ri].ranges.splice(gi, 1))}>✕</button>}
                        </span>
                      ))}
                      {!readOnly && <button type="button" className="btn" onClick={() => upd((d) => d.schedule.seasons[si].rules[ri].ranges.push({ start: "00:00", end: "06:00" }))}>Add range</button>}
                      {!readOnly && <button type="button" className="btn" onClick={() => upd((d) => d.schedule.seasons[si].rules.splice(ri, 1))}>Remove rule</button>}
                    </Row>
                  </div>
                ))}
                {!readOnly && (
                  <div className="controls">
                    <button type="button" className="btn" onClick={() => upd((d) => d.schedule.seasons[si].rules.push({ days: ["all"], ranges: [{ start: "00:00", end: "24:00" }], period: d.periods[0].id }))}>Add rule</button>
                    <span className="muted">Overnight helper:</span>
                    <input type="time" aria-label="Overnight from" value={overnight.start} onChange={(e) => setOvernight({ ...overnight, start: e.target.value })} />
                    <input type="time" aria-label="Overnight until" value={overnight.end} onChange={(e) => setOvernight({ ...overnight, end: e.target.value })} />
                    <button type="button" className="btn" onClick={() => upd((d) => d.schedule.seasons[si].rules.push({ days: ["all"], ranges: overnightRanges(overnight.start, overnight.end), period: d.periods[d.periods.length - 1].id }))}>Add as a rule</button>
                  </div>
                )}
              </div>
            ))}
            {!readOnly && <button type="button" className="btn" onClick={() => upd((d) => d.schedule.seasons.push({ id: `season_${d.schedule.seasons.length + 1}`, from: "01-01", to: "12-31", rules: [{ days: ["all"], ranges: [{ start: "00:00", end: "24:00" }], period: d.periods[0].id }] }))}>Add season</button>}

            <h3 className="sub">Public holidays (optional) <InfoTip text="Dates that follow the 'holiday' rules instead of the weekday rules. Many tariffs, like Bulgaria's regulated one, do not treat holidays differently." /></h3>
            <textarea rows={2} aria-label="Holidays" placeholder="2026-01-01, 2026-03-03" value={holidayText} disabled={readOnly} onChange={(e) => { setHolidayText(e.target.value); const p = parseHolidays(e.target.value); upd((d) => (d.holidays = p.dates)); }} />
            {holidays.bad.length > 0 && <span className="note err">Not a date (use YYYY-MM-DD): {holidays.bad.join(", ")}</span>}

            <h3 className="sub">Check</h3>
            {preview && <Notice kind="info">Browser preview: live validation and saving run inside the PowerCost app.</Notice>}
            {report && !report.ok && <Notice kind="error"><b>Fix these before saving:</b><ul>{report.errors.map((e) => <li key={e}>{e}</li>)}</ul></Notice>}
            {report?.ok && (
              <>
                <table className="rows" style={{ maxWidth: 420 }}>
                  <thead><tr><th>Resulting price per kWh</th><th className="n">excl. VAT</th><th className="n">you pay</th></tr></thead>
                  <tbody>{report.prices.map((p) => <tr key={p.period_id}><td>{p.label}</td><td className="n">{p.excl_vat}</td><td className="n"><b>{p.gross}</b> {doc.currency}</td></tr>)}</tbody>
                </table>
                {report.warnings.map((w) => <Notice kind="info" key={w.code}>{w.message}</Notice>)}
              </>
            )}

            {!readOnly && (
              <div className="savebar">
                {isNew ? (
                  <button type="button" className="btn primary" disabled={!canSave} onClick={() => persist(doc, null)}>Save new tariff</button>
                ) : (
                  <>
                    <button type="button" className="btn primary" disabled={!canSave} onClick={() => persist(doc, null)} title="Overwrite this version, for fixing a mistake">Save corrections to v{doc.version}</button>
                    <span className="controls" style={{ margin: 0 }}>
                      or prices changed on <input type="date" aria-label="New version valid from" value={newFrom} onChange={(e) => setNewFrom(e.target.value)} />
                      <button type="button" className="btn" disabled={!report?.ok || newFrom <= doc.valid_from} onClick={() => persist(nextVersion(doc, newFrom), null)}>Save as new version</button>
                      <InfoTip text="A new version applies from its start date; earlier usage keeps the old prices. If this is your active tariff, usage since that date is repriced right away." />
                    </span>
                  </>
                )}
              </div>
            )}
            {msg && <Notice kind={msg.kind}>{msg.text.map((t) => <div key={t}>{t}</div>)}</Notice>}
          </>
        )}
      </section>
    </div>
  );
}
