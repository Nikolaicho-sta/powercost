import { useEffect, useState } from "react";
import { api, errorLines, type AppInfo, type PowerStateName, type PresetInfo, type Settings, type Theme, type UpdateStatus } from "../api";
import { UpdateBox } from "./UpdateBox";
import { formatMinor, parseMinor } from "../money";
import { InfoTip } from "./InfoTip";
import { Segmented } from "./Segmented";
import { Check, Dec, Field, IntInput, Notice, verificationBadge } from "./ui";

const GAP_STATES: { value: PowerStateName; label: string }[] = [
  { value: "off", label: "Off (shut down)" },
  { value: "idle", label: "Idle (still on)" },
  { value: "sleep", label: "Sleep" },
];

function isoDate(d: Date) {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export function SettingsPage({ info, update, onUpdate, onSaved, onCalibrate, onOpenTariffs }: { info: AppInfo | null; update: UpdateStatus; onUpdate: (s: UpdateStatus) => void; onSaved: (s: Settings) => void; onCalibrate: () => void; onOpenTariffs: () => void }) {
  const [saved, setSaved] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [tariffs, setTariffs] = useState<PresetInfo[]>([]);
  const [errors, setErrors] = useState<string[]>([]);
  const [status, setStatus] = useState<string | null>(null);
  const [budgetText, setBudgetText] = useState("");
  const [percentsText, setPercentsText] = useState("80, 100");
  const [exp, setExp] = useState({ from: isoDate(new Date(Date.now() - 30 * 864e5)), to: isoDate(new Date()), format: "csv" as "csv" | "json", granularity: "day" as "hour" | "day" });
  const [expMsg, setExpMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [repriceDays, setRepriceDays] = useState(30);

  useEffect(() => {
    api.getSettings().then((s) => {
      setSaved(s);
      setDraft(s);
      setBudgetText(s.budget ? formatMinor(s.budget.monthly_minor, 2) : "");
      setPercentsText((s.budget?.alert_percents ?? [80, 100]).join(", "));
    });
    api.listTariffs().then((l) => {
      const latest = new Map<string, PresetInfo>();
      for (const p of l.presets) {
        const cur = latest.get(p.id);
        if (!cur || p.valid_from >= cur.valid_from) latest.set(p.id, p);
      }
      setTariffs([...latest.values()]);
    });
  }, []);

  if (!draft || !saved) return <p className="muted">Loading...</p>;
  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  const upd = (f: (d: Settings) => void) => {
    const c = structuredClone(draft);
    f(c);
    setDraft(c);
    setStatus(null);
  };

  async function save() {
    if (!draft) return;
    const next = structuredClone(draft);
    const problems: string[] = [];
    if (budgetText.trim() === "") {
      next.budget = null;
    } else {
      const minor = parseMinor(budgetText, 2);
      const pct = percentsText.split(/[,\s]+/).filter(Boolean).map(Number);
      if (minor === null || minor <= 0) problems.push("Monthly budget must be an amount like 25.00 (at most 2 decimals).");
      if (pct.length === 0 || pct.some((p) => !Number.isInteger(p) || p < 1 || p > 1000)) problems.push("Budget alerts must be whole percentages like 80, 100.");
      if (!problems.length) next.budget = { monthly_minor: minor!, alert_percents: pct };
    }
    if (problems.length) {
      setErrors(problems);
      return;
    }
    try {
      const s = await api.saveSettings(next);
      setSaved(s);
      setDraft(s);
      setErrors([]);
      setStatus("Saved. Changes apply immediately.");
      onSaved(s);
    } catch (e) {
      setErrors(errorLines(e));
    }
  }

  const e = draft.estimator;
  const th = draft.thresholds;
  const setEst = (f: (e: Settings["estimator"]) => void) => upd((d) => f(d.estimator));

  async function doExport() {
    setExpMsg(null);
    try {
      const path = await api.pickSavePath(`powercost-${exp.from}_${exp.to}.${exp.format}`, exp.format);
      if (!path) {
        setExpMsg({ ok: false, text: info?.preview ? "Exports run inside the PowerCost app." : "Export cancelled." });
        return;
      }
      const written = await api.exportData({ path, format: exp.format, granularity: exp.granularity, fromDate: exp.from, toDate: exp.to });
      setExpMsg({ ok: true, text: `Saved to ${written}` });
    } catch (err) {
      setExpMsg({ ok: false, text: errorLines(err).join(" ") });
    }
  }

  return (
    <div className="grid">
      <section className="card span-6" aria-labelledby="s-general">
        <h2 id="s-general">General</h2>
        <div className="form">
          <Field label="Theme">
            <Segmented<Theme> label="Theme" value={draft.theme} onChange={(v) => upd((d) => (d.theme = v))} options={[{ value: "system", label: "System" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} />
          </Field>
          <Check label="Start with Windows (in the tray, no window)" checked={draft.autostart} onChange={(v) => upd((d) => (d.autostart = v))} help="Adds PowerCost to your per-user startup list (no admin rights). Turn it off here at any time." />
          <Field label="Region (country code)" help="Used to suggest tariffs. It is detected from your language and time zone and never sent anywhere unless you opt in to the IP lookup in the first-run wizard.">
            <input value={draft.region.country ?? ""} maxLength={2} style={{ width: 70, textTransform: "uppercase" }} onChange={(ev) => upd((d) => { d.region.country = ev.target.value.toUpperCase() || null; d.region.confirmed = true; })} />
          </Field>
          <Field label="Tariff">
            <select value={draft.tariff_id} onChange={(ev) => upd((d) => (d.tariff_id = ev.target.value))}>
              {tariffs.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.country} - {t.name}
                  {t.source === "user" ? " (mine)" : ""}
                </option>
              ))}
              {!tariffs.some((t) => t.id === draft.tariff_id) && <option value={draft.tariff_id}>{draft.tariff_id} (not found)</option>}
            </select>
            <span className="note">
              {verificationBadge(tariffs.find((t) => t.id === draft.tariff_id)?.verification)}{" "}
              <button type="button" className="link" onClick={onOpenTariffs}>Edit or add tariffs</button>
            </span>
          </Field>
          <Check label="Include fixed monthly fees in totals" checked={draft.include_fixed_fees} onChange={(v) => upd((d) => (d.include_fixed_fees = v))} help="Fixed fees (meter, subscription) are spread evenly over the days of each month. Off by default because they do not depend on how you use the PC." />
          <div className="controls">
            Reprice the last
            <IntInput value={repriceDays} onChange={setRepriceDays} min={1} max={90} ariaLabel="Days to reprice" /> days with the selected tariff
            <button type="button" className="btn" disabled={dirty} title={dirty ? "Save first" : ""} onClick={async () => {
              try {
                const n = await api.repriceHistory(repriceDays);
                setStatus(`Repriced ${n} stretches of usage.`);
              } catch (err) {
                setErrors(errorLines(err));
              }
            }}>Reprice</button>
            <InfoTip text="Switching tariff does not rewrite past costs by itself. This re-derives them from the stored usage. Days already compacted into daily totals cannot be repriced." />
          </div>
        </div>
      </section>

      <section className="card span-6" aria-labelledby="s-power">
        <h2 id="s-power">
          Power estimation
          <InfoTip text="How watts are estimated when the PC is on. A wall-meter calibration (below) is the most accurate; otherwise measured CPU/GPU power plus a modelled rest of system, or a default model from your hardware." />
        </h2>
        <div className="form two">
          <Field label="Power supply / adapter efficiency" help="Wall power = component power / efficiency. 0.85 is typical for a desktop PSU, about 0.90 for a laptop adapter."><Dec value={e.psu_efficiency} onChange={(v) => setEst((x) => (x.psu_efficiency = v))} width={80} /></Field>
          <Field label="Rest of system" help="Board, RAM, fans and display panel, excluding CPU, GPU and drives."><Dec value={e.baseline_w} onChange={(v) => setEst((x) => (x.baseline_w = v))} unit="W" width={70} /></Field>
          <Field label="Per drive"><Dec value={e.per_drive_w} onChange={(v) => setEst((x) => (x.per_drive_w = v))} unit="W" width={70} /></Field>
          <Field label="Number of drives"><IntInput value={e.hardware.drives} onChange={(v) => setEst((x) => (x.hardware.drives = v))} min={0} max={32} /></Field>
          <Field label="CPU power at full load (TDP)"><Dec value={e.hardware.cpu_tdp_w} onChange={(v) => setEst((x) => (x.hardware.cpu_tdp_w = v))} unit="W" width={70} /></Field>
          <Field label="CPU power at idle"><Dec value={e.hardware.cpu_idle_w} onChange={(v) => setEst((x) => (x.hardware.cpu_idle_w = v))} unit="W" width={70} /></Field>
          <Field label="Graphics card TDP" hint="Leave empty if there is no separate GPU."><Dec value={e.hardware.gpu_tdp_w ?? ""} onChange={(v) => setEst((x) => (x.hardware.gpu_tdp_w = v.trim() === "" ? null : v))} unit="W" width={70} /></Field>
          <Field label="Graphics card at idle"><Dec value={e.hardware.gpu_idle_w} onChange={(v) => setEst((x) => (x.hardware.gpu_idle_w = v))} unit="W" width={70} /></Field>
        </div>
        <div className="form">
          <Check label="Count monitor power" checked={e.include_monitor} onChange={(v) => setEst((x) => (x.include_monitor = v))} help="Adds the monitor's draw while you are ACTIVE (monitors sleep when idle). Off by default; not needed for a laptop's built-in screen if you calibrated with it on." />
          {e.include_monitor && <Field label="Monitor"><Dec value={e.monitor_w} onChange={(v) => setEst((x) => (x.monitor_w = v))} unit="W" width={70} /></Field>}
          <div className="form three">
            <Field label="Sleep"><Dec value={e.sleep_w} onChange={(v) => setEst((x) => (x.sleep_w = v))} unit="W" width={60} /></Field>
            <Field label="Hibernate"><Dec value={e.hibernate_w} onChange={(v) => setEst((x) => (x.hibernate_w = v))} unit="W" width={60} /></Field>
            <Field label="Shut down (standby)" help="What a shut-down PC still draws (PSU and board standby), roughly 0.5 to 2 W."><Dec value={e.off_standby_w} onChange={(v) => setEst((x) => (x.off_standby_w = v))} unit="W" width={60} /></Field>
          </div>
          <Check label="I switch it off at the wall when shut down (0 W)" checked={e.unplugged_when_off} onChange={(v) => setEst((x) => (x.unplugged_when_off = v))} />
          <Check label="Prefer my wattmeter calibration over sensors" checked={e.prefer_calibrated} onChange={(v) => setEst((x) => (x.prefer_calibrated = v))} help="A wall meter measures the whole PC; CPU/GPU sensors cover only part of it. Untick to use sensors first." />
          <div className="controls">
            {e.calibration ? <span>Calibrated: {e.calibration.points[0].watts} W idle, {e.calibration.points[e.calibration.points.length - 1].watts} W full load.</span> : <span className="muted">Not calibrated.</span>}
            <button type="button" className="btn" onClick={onCalibrate}>{e.calibration ? "Redo calibration" : "Calibrate with a wattmeter"}</button>
          </div>
        </div>
      </section>

      <section className="card span-6" aria-labelledby="s-thresholds">
        <h2 id="s-thresholds">
          Activity detection
          <InfoTip text="ACTIVE means recent keyboard/mouse input or meaningful load. IDLE is on with no input and low load. Load uses two levels so brief dips do not flip the state." />
        </h2>
        <div className="form two">
          <Field label="Idle after no input for"><IntInput value={th.input_idle_secs} onChange={(v) => upd((d) => (d.thresholds.input_idle_secs = v))} unit="s" min={10} max={86400} /></Field>
          <Field label="Samples before leaving active"><IntInput value={th.exit_debounce_ticks} onChange={(v) => upd((d) => (d.thresholds.exit_debounce_ticks = v))} min={1} max={20} /></Field>
          <Field label="CPU load to become active"><IntInput value={th.cpu_enter_percent} onChange={(v) => upd((d) => (d.thresholds.cpu_enter_percent = v))} unit="%" min={0} max={100} /></Field>
          <Field label="CPU load to stay active"><IntInput value={th.cpu_exit_percent} onChange={(v) => upd((d) => (d.thresholds.cpu_exit_percent = v))} unit="%" min={0} max={100} /></Field>
          <Field label="GPU load to become active"><IntInput value={th.gpu_enter_percent} onChange={(v) => upd((d) => (d.thresholds.gpu_enter_percent = v))} unit="%" min={0} max={100} /></Field>
          <Field label="GPU load to stay active"><IntInput value={th.gpu_exit_percent} onChange={(v) => upd((d) => (d.thresholds.gpu_exit_percent = v))} unit="%" min={0} max={100} /></Field>
        </div>
        <h3 className="sub">Unexplained time</h3>
        <div className="form two">
          <Field label="After a crash or power loss assume" help="When the PC lost power the app cannot know when. This is the state assumed for the whole unexplained stretch."><select value={draft.gap_assumptions.crash} onChange={(ev) => upd((d) => (d.gap_assumptions.crash = ev.target.value as PowerStateName))}>{GAP_STATES.map((g) => <option key={g.value} value={g.value}>{g.label}</option>)}</select></Field>
          <Field label="When nothing is known assume" help="Used when the Windows event log has no record for a gap (for example because it rolled over)."><select value={draft.gap_assumptions.unknown} onChange={(ev) => upd((d) => (d.gap_assumptions.unknown = ev.target.value as PowerStateName))}>{GAP_STATES.map((g) => <option key={g.value} value={g.value}>{g.label}</option>)}</select></Field>
        </div>
      </section>

      <section className="card span-6" aria-labelledby="s-budget">
        <h2 id="s-budget">Budget alerts</h2>
        <div className="form">
          <Field label="Monthly budget" hint={`Leave empty for no budget. Amounts are in ${"your tariff's currency"}.`}>
            <input inputMode="decimal" value={budgetText} placeholder="e.g. 15.00" style={{ width: 110 }} onChange={(ev) => { setBudgetText(ev.target.value); setStatus(null); }} />
          </Field>
          <Field label="Notify at (percent of budget)" help="One notification per threshold per month, for the highest threshold reached."><input value={percentsText} style={{ width: 130 }} onChange={(ev) => { setPercentsText(ev.target.value); setStatus(null); }} /></Field>
          <div className="controls">
            <button type="button" className="btn" onClick={async () => { try { await api.testNotification(); setStatus("Test notification sent."); } catch (err) { setErrors(errorLines(err)); } }}>Send a test notification</button>
          </div>
        </div>
        <h3 className="sub">Data</h3>
        <div className="form">
          <Field label="Keep minute-level detail for" help="Older data is compacted into hourly and daily totals to keep the database small. Compacted days can no longer be repriced or re-estimated."><IntInput value={draft.detail_retention_days} onChange={(v) => upd((d) => (d.detail_retention_days = v))} unit="days" min={7} max={3650} /></Field>
        </div>
      </section>

      <section className="card span-12" aria-labelledby="s-export">
        <h2 id="s-export">
          Export
          <InfoTip text="Writes your estimated usage and cost per hour or day, split by power state and tariff period. Amounts are exact decimals; the JSON file also records the tariff and a disclaimer that every figure is an estimate." />
        </h2>
        <div className="controls">
          <label>from <input type="date" value={exp.from} onChange={(ev) => setExp({ ...exp, from: ev.target.value })} /></label>
          <label>to <input type="date" value={exp.to} onChange={(ev) => setExp({ ...exp, to: ev.target.value })} /></label>
          <Segmented<"day" | "hour"> label="Granularity" value={exp.granularity} onChange={(v) => setExp({ ...exp, granularity: v })} options={[{ value: "day", label: "Per day" }, { value: "hour", label: "Per hour" }]} />
          <Segmented<"csv" | "json"> label="Format" value={exp.format} onChange={(v) => setExp({ ...exp, format: v })} options={[{ value: "csv", label: "CSV" }, { value: "json", label: "JSON" }]} />
          <button type="button" className="btn primary" onClick={doExport}>Export...</button>
        </div>
        {expMsg && <Notice kind={expMsg.ok ? "ok" : "info"}>{expMsg.text}</Notice>}
      </section>

      <section className="card span-12" aria-labelledby="s-update">
        <h2 id="s-update">
          Updates
          <InfoTip text="Updating rebuilds PowerCost from the project folder and installs it over this copy. Your data and settings are kept. The check never goes online." />
        </h2>
        <UpdateBox status={update} mode={info?.update_mode ?? "folder"} onStatus={onUpdate} />
        {info?.update_mode === "github" && (
          <div className="form" style={{ marginTop: 12 }}>
            <Check
              label="Look for updates automatically (about once a day)"
              checked={draft.online_update_check}
              onChange={(v) => upd((d) => (d.online_update_check = v))}
              help="Off by default. When on, PowerCost asks github.com once a day whether a newer release exists and shows a notification. It sends nothing about you or your usage. Remember to press Save settings."
            />
          </div>
        )}
      </section>

      <p className="note span-12" style={{ margin: 0 }}>
        PowerCost {info?.version ?? ""}
        {info?.build ? ` · build ${info.build}` : ""}
        {info?.data_dir ? ` · data in ${info.data_dir}` : ""}.
      </p>

      <div className="savebar span-12">
        {errors.length > 0 && (
          <Notice kind="error">
            <ul>{errors.map((m) => <li key={m}>{m}</li>)}</ul>
          </Notice>
        )}
        {status && !errors.length && <Notice kind="ok">{status}</Notice>}
        <button type="button" className="btn primary" disabled={!dirty && budgetText === (saved.budget ? formatMinor(saved.budget.monthly_minor, 2) : "") && percentsText === (saved.budget?.alert_percents ?? [80, 100]).join(", ")} onClick={save}>Save settings</button>
        <button type="button" className="btn" disabled={!dirty} onClick={() => { setDraft(saved); setErrors([]); setStatus(null); }}>Discard changes</button>
      </div>
    </div>
  );
}
