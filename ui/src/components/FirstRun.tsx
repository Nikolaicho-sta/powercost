import { useEffect, useState } from "react";
import { api, errorLines, type PresetInfo, type RegionInfo, type Settings } from "../api";
import { lookupRegion, IP_LOOKUP_URL } from "../ipLookup";
import { Check, Field, Modal, Notice, verificationBadge } from "./ui";

const STEPS = ["Welcome", "Region", "Tariff", "Your PC", "Done"] as const;

function systemLocale() {
  try {
    return { locale: navigator.language || "", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "" };
  } catch {
    return { locale: "", timezone: "" };
  }
}

export function FirstRunWizard({ initial, onDone }: { initial: Settings; onDone: (s: Settings, openCalibration: boolean, openTariffs: boolean) => void }) {
  const [step, setStep] = useState(0);
  const [region, setRegion] = useState<RegionInfo | null>(null);
  const [country, setCountry] = useState<string>(initial.region.country ?? "");
  const [ipOptIn, setIpOptIn] = useState(false);
  const [ipMsg, setIpMsg] = useState<string | null>(null);
  const [presets, setPresets] = useState<PresetInfo[]>([]);
  const [tariffId, setTariffId] = useState<string>(initial.tariff_id);
  const [own, setOwn] = useState(false);
  const [kind, setKind] = useState<"laptop" | "desktop">("laptop");
  const [monitor, setMonitor] = useState(false);
  const [wall, setWall] = useState(false);
  const [errors, setErrors] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const { locale, timezone } = systemLocale();
    api.detectRegion(locale, timezone).then((r) => {
      setRegion(r);
      if (!country && r.candidates[0]) setCountry(r.candidates[0].country);
    });
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (country.length === 2) {
      api.presetsForCountry(country).then((p) => {
        const latest = new Map<string, PresetInfo>();
        for (const x of p) {
          const cur = latest.get(x.id);
          if (!cur || x.valid_from >= cur.valid_from) latest.set(x.id, x);
        }
        const list = [...latest.values()];
        setPresets(list);
        if (list.length && !list.some((l) => l.id === tariffId)) setTariffId(list[0].id);
      });
    } else {
      setPresets([]);
    }
  }, [country]); // eslint-disable-line react-hooks/exhaustive-deps

  async function doLookup() {
    setIpMsg("Looking up...");
    try {
      const r = await lookupRegion();
      setCountry(r.country);
      setIpMsg(`The service says: ${r.country}. Check it below - you can still change it.`);
    } catch (e) {
      setIpMsg(`Lookup failed (${errorLines(e).join(" ")}). Pick your country manually.`);
    }
  }

  async function finish(openCalibration: boolean, openTariffs: boolean) {
    setBusy(true);
    const s = structuredClone(initial);
    s.first_run_done = true;
    s.region = { country: country || null, confirmed: true, ip_lookup_opt_in: ipOptIn };
    if (!own && presets.some((p) => p.id === tariffId)) s.tariff_id = tariffId;
    s.estimator.psu_efficiency = kind === "laptop" ? "0.90" : "0.85";
    s.estimator.baseline_w = kind === "laptop" ? "8" : "20";
    s.estimator.include_monitor = monitor;
    s.estimator.unplugged_when_off = wall;
    try {
      const saved = await api.saveSettings(s);
      onDone(saved, openCalibration, openTariffs || own);
    } catch (e) {
      setErrors(errorLines(e));
      setBusy(false);
    }
  }

  const canNext = [true, country.length === 2, own || presets.some((p) => p.id === tariffId) || presets.length === 0, true][step] ?? true;

  return (
    <Modal title="Welcome to PowerCost">
      <ol className="steps" aria-label="Setup steps">
        {STEPS.map((s, i) => (
          <li key={s} className={i === step ? "on" : i < step ? "done" : ""}>{s}</li>
        ))}
      </ol>

      {step === 0 && (
        <div className="prose">
          <p>PowerCost estimates what your PC costs in electricity, by power state (active, idle, sleep, off) and by tariff period (for example day and night). Setup takes about a minute.</p>
          <ul>
            <li><b>Everything stays on this computer.</b> There is no account, no telemetry.</li>
            <li><b>Every number is an estimate.</b> The app shows how each was obtained and how confident it is.</li>
            <li><b>The only network use is optional:</b> a one-time lookup of your country, which you can switch on below. Nothing else leaves your PC.</li>
          </ul>
        </div>
      )}

      {step === 1 && (
        <div className="prose">
          <p>Your region decides which electricity tariffs we suggest. We guessed it from your language and time zone - please confirm or change it.</p>
          <div className="controls">
            {region?.candidates.map((c) => (
              <label key={c.country} className="radio">
                <input type="radio" name="cand" checked={country === c.country} onChange={() => setCountry(c.country)} /> {c.country} <span className="muted">(from your {c.source === "timezone" ? "time zone" : "language"})</span>
              </label>
            ))}
            {region && region.candidates.length === 0 && <span className="muted">We could not guess your country.</span>}
          </div>
          <Field label="Country code" hint="Two letters, for example BG, DE, GB, FR.">
            <input value={country} maxLength={2} style={{ width: 70, textTransform: "uppercase" }} onChange={(e) => setCountry(e.target.value.toUpperCase())} />
          </Field>
          <div className="optin">
            <Check label="Let PowerCost look up my country from my IP address (optional)" checked={ipOptIn} onChange={setIpOptIn} />
            <p className="note">If you tick this and press the button, your browser asks <code>{IP_LOOKUP_URL}</code>, which sees your IP address as any website would. Nothing else is sent. Skip it and nothing goes online.</p>
            <button type="button" className="btn" disabled={!ipOptIn} onClick={doLookup}>Look up my country</button>
            {ipMsg && <p className="note">{ipMsg}</p>}
          </div>
        </div>
      )}

      {step === 2 && (
        <div className="prose">
          {presets.length > 0 ? (
            <>
              <p>Tariffs we ship for <b>{country}</b>. Prices change (in Bulgaria usually around 1 July), so you can correct them later in the tariff editor.</p>
              <div className="cards">
                {presets.map((p) => (
                  <label key={p.id} className={`pick${tariffId === p.id && !own ? " on" : ""}`}>
                    <input type="radio" name="tariff" checked={tariffId === p.id && !own} onChange={() => { setTariffId(p.id); setOwn(false); }} />
                    <span>
                      <b>{p.name}</b>
                      <span className="note">{p.distributor ? `${p.distributor} - ` : ""}valid from {p.valid_from} {verificationBadge(p.verification)}</span>
                    </span>
                  </label>
                ))}
              </div>
              {presets.find((p) => p.id === tariffId)?.verification !== "verified" && !own && <Notice kind="info">This preset is not fully verified against an official source. Check the prices against your bill in the tariff editor.</Notice>}
            </>
          ) : (
            <Notice kind="info">There is no preset for {country || "your country"} yet. You can create your own tariff (a flat price takes a minute), or drop a tariff JSON file into the tariffs folder.</Notice>
          )}
          <Check label="I will enter my own tariff after setup" checked={own} onChange={setOwn} />
        </div>
      )}

      {step === 3 && (
        <div className="prose">
          <p>A few details improve the estimate. You can change them any time in Settings.</p>
          <div className="controls">
            <label className="radio"><input type="radio" name="kind" checked={kind === "laptop"} onChange={() => setKind("laptop")} /> Laptop</label>
            <label className="radio"><input type="radio" name="kind" checked={kind === "desktop"} onChange={() => setKind("desktop")} /> Desktop</label>
          </div>
          <Check label="Count my monitor's power while I am active" checked={monitor} onChange={setMonitor} help="Off by default. Tick it if you use an external monitor you want included." />
          <Check label="I switch the PC off at the wall when it is shut down (0 W)" checked={wall} onChange={setWall} />
        </div>
      )}

      {step === 4 && (
        <div className="prose">
          <p>All set. PowerCost will start tracking in the background (look for its icon in the tray) and the dashboard fills in as time passes.</p>
          <p>For a much more accurate estimate, measure your PC once with a plug-in wattmeter - it takes about ten minutes.</p>
          {errors.length > 0 && <Notice kind="error"><ul>{errors.map((m) => <li key={m}>{m}</li>)}</ul></Notice>}
        </div>
      )}

      <div className="modal-foot">
        <button type="button" className="btn" disabled={step === 0 || busy} onClick={() => setStep(step - 1)}>Back</button>
        <span style={{ flex: 1 }} />
        {step < 4 ? (
          <button type="button" className="btn primary" disabled={!canNext} onClick={() => setStep(step + 1)}>Next</button>
        ) : (
          <>
            <button type="button" className="btn" disabled={busy} onClick={() => finish(true, false)}>Finish and calibrate</button>
            <button type="button" className="btn primary" disabled={busy} onClick={() => finish(false, false)}>Finish</button>
          </>
        )}
      </div>
    </Modal>
  );
}
