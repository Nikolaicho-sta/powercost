// Helpers for the tariff editor. The document shape mirrors schema/tariff.schema.json; all
// validation and price maths stay in Rust (the editor calls the validate command).

export type Verification = "verified" | "partial" | "unverified" | "example";
export type DayKind = "all" | "weekday" | "weekend" | "saturday" | "sunday" | "holiday";

export interface Component {
  id: string;
  kind: "energy" | "network" | "excise" | "levy" | "other";
  label?: string;
  vat_inclusive: boolean;
  per_kwh?: string;
  per_period?: Record<string, string>;
  source_url?: string | null;
}
export interface FixedFee { id: string; label?: string; per_month: string; vat_inclusive: boolean }
export interface Period { id: string; label: string }
export interface TimeRange { start: string; end: string }
export interface Rule { days: DayKind[]; ranges: TimeRange[]; period: string }
export interface Season { id: string; from: string; to: string; rules: Rule[] }

export interface TariffDoc {
  schema_version: 1;
  id: string;
  version: number;
  name: string;
  country: string;
  distributor?: string | null;
  currency: string;
  timezone: string;
  vat_percent: string;
  valid_from: string;
  valid_to?: string | null;
  source_url?: string | null;
  last_verified?: string | null;
  verification?: Verification;
  notes?: string;
  components: Component[];
  fixed_fees: FixedFee[];
  periods: Period[];
  holidays: string[];
  schedule: { seasons: Season[] };
}

export function slugify(name: string): string {
  const s = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  return s || "tariff";
}

/** A new empty flat tariff the user can fill in. */
export function blankTariff(country: string, currency: string, timezone: string, today: string): TariffDoc {
  return {
    schema_version: 1,
    id: "user-my-tariff",
    version: 1,
    name: "My tariff",
    country,
    distributor: null,
    currency,
    timezone,
    vat_percent: "0",
    valid_from: today,
    valid_to: null,
    source_url: null,
    last_verified: null,
    verification: "unverified",
    components: [{ id: "energy", kind: "energy", label: "Energy price", vat_inclusive: true, per_kwh: "0.10" }],
    fixed_fees: [],
    periods: [{ id: "flat", label: "Flat" }],
    holidays: [],
    schedule: { seasons: [{ id: "all", from: "01-01", to: "12-31", rules: [{ days: ["all"], ranges: [{ start: "00:00", end: "24:00" }], period: "flat" }] }] },
  };
}

/** A copy of a (bundled) tariff as the user's own: new id, version 1, marked unverified until checked. */
export function copyAsUser(t: TariffDoc, today: string): TariffDoc {
  const copy: TariffDoc = JSON.parse(JSON.stringify(t));
  const base = t.id.startsWith("user-") ? t.id : `user-${t.id}`;
  copy.id = base;
  copy.version = 1;
  copy.valid_from = today;
  copy.valid_to = null;
  copy.verification = "unverified";
  copy.last_verified = null;
  return copy;
}

/** The next version of a user tariff: same id, version + 1, starting on `validFrom` (e.g. 1 July). */
export function nextVersion(t: TariffDoc, validFrom: string): TariffDoc {
  const copy: TariffDoc = JSON.parse(JSON.stringify(t));
  copy.version = t.version + 1;
  copy.valid_from = validFrom;
  copy.valid_to = null;
  return copy;
}

/** Ranges never wrap midnight in the data format: an overnight window becomes two ranges. */
export function overnightRanges(start: string, end: string): TimeRange[] {
  if (start === end) return [{ start: "00:00", end: "24:00" }];
  if (start < end) return [{ start, end }];
  const out: TimeRange[] = [{ start, end: "24:00" }];
  if (end !== "00:00") out.unshift({ start: "00:00", end });
  return out;
}

/** True when every minute of the day is covered exactly once (quick client-side hint; Rust is the authority). */
export function rangesCoverDay(ranges: TimeRange[]): boolean {
  const toMin = (t: string) => Number(t.slice(0, 2)) * 60 + Number(t.slice(3, 5));
  const sorted = [...ranges].map((r) => [toMin(r.start), toMin(r.end)] as const).sort((a, b) => a[0] - b[0]);
  let cursor = 0;
  for (const [s, e] of sorted) {
    if (s !== cursor || e <= s) return false;
    cursor = e;
  }
  return cursor === 1440;
}

export function isDecimalString(s: string): boolean {
  return /^-?\d+(\.\d+)?$/.test(s.trim());
}

/** Rename a period id everywhere it is referenced (component prices and schedule rules). */
export function renamePeriod(t: TariffDoc, oldId: string, newId: string): TariffDoc {
  const c: TariffDoc = JSON.parse(JSON.stringify(t));
  for (const p of c.periods) if (p.id === oldId) p.id = newId;
  for (const comp of c.components) {
    if (comp.per_period && oldId in comp.per_period) {
      const rebuilt: Record<string, string> = {};
      for (const [k, v] of Object.entries(comp.per_period)) rebuilt[k === oldId ? newId : k] = v;
      comp.per_period = rebuilt;
    }
  }
  for (const s of c.schedule.seasons) for (const r of s.rules) if (r.period === oldId) r.period = newId;
  return c;
}

/** Remove a period: its prices go, and the rules that pointed at it are dropped (the editor then
 *  shows the uncovered hours so the user can fix the schedule). */
export function removePeriod(t: TariffDoc, id: string): TariffDoc {
  const c: TariffDoc = JSON.parse(JSON.stringify(t));
  c.periods = c.periods.filter((p) => p.id !== id);
  for (const comp of c.components) if (comp.per_period) delete comp.per_period[id];
  for (const s of c.schedule.seasons) s.rules = s.rules.filter((r) => r.period !== id);
  return c;
}

export function addPeriod(t: TariffDoc, id: string, label: string): TariffDoc {
  const c: TariffDoc = JSON.parse(JSON.stringify(t));
  c.periods.push({ id, label });
  for (const comp of c.components) {
    if (comp.per_period) comp.per_period[id] = Object.values(comp.per_period)[0] ?? "0";
  }
  return c;
}

/** A component is either one price for every period or one price per period (never both). */
export function setComponentMode(comp: Component, mode: "flat" | "per_period", periods: Period[]): Component {
  const c: Component = { ...comp };
  if (mode === "flat" && c.per_period) {
    c.per_kwh = Object.values(c.per_period)[0] ?? "0";
    delete c.per_period;
  } else if (mode === "per_period" && !c.per_period) {
    const base = c.per_kwh ?? "0";
    c.per_period = Object.fromEntries(periods.map((p) => [p.id, base]));
    delete c.per_kwh;
  }
  return c;
}

export function parseHolidays(text: string): { dates: string[]; bad: string[] } {
  const parts = text.split(/[\s,;]+/).filter(Boolean);
  const dates = parts.filter((p) => /^\d{4}-\d{2}-\d{2}$/.test(p));
  return { dates: [...new Set(dates)].sort(), bad: parts.filter((p) => !/^\d{4}-\d{2}-\d{2}$/.test(p)) };
}

/** Users in many locales type a decimal comma; the data format wants a dot. */
export const dot = (s: string): string => s.replace(',', '.');
