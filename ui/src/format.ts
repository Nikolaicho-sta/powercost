import type { Amount, PowerStateName } from "./api";

const NANO = 1e9;

/** Display only: Rust computed the numbers; this just formats them. */
export function money(minor: number, currency: string, digits: number): string {
  return new Intl.NumberFormat(undefined, { style: "currency", currency, minimumFractionDigits: digits, maximumFractionDigits: digits }).format(
    minor / 10 ** digits,
  );
}

/** Sub-cent amounts (single hours, single states) need more decimals. */
export function moneyPrecise(nano: number, currency: string): string {
  const v = nano / NANO;
  const digits = Math.abs(v) >= 1 ? 2 : Math.abs(v) >= 0.01 ? 3 : 4;
  return new Intl.NumberFormat(undefined, { style: "currency", currency, minimumFractionDigits: digits, maximumFractionDigits: digits }).format(v);
}

export function kwh(nano: number): string {
  const v = nano / NANO;
  const digits = v >= 100 ? 0 : v >= 10 ? 1 : 2;
  return `${v.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits })} kWh`;
}

export function hours(seconds: number): string {
  const h = seconds / 3600;
  return h >= 100 ? `${Math.round(h)} h` : `${h.toFixed(1)} h`;
}

/** Share in per-mille of `part` within `whole` by the given field. */
export function share(part: Amount, whole: Amount, field: "cost_nano" | "kwh_nano" = "cost_nano"): number {
  return whole[field] > 0 ? Math.round((part[field] / whole[field]) * 1000) : 0;
}

export function percent(permille: number): string {
  return `${(permille / 10).toFixed(permille % 10 === 0 ? 0 : 1)}%`;
}

export const STATE_LABEL: Record<PowerStateName, string> = {
  active: "Active",
  idle: "Idle",
  sleep: "Sleep",
  hibernate: "Hibernate",
  off: "Off",
};

export const STATE_HELP: Record<PowerStateName, string> = {
  active: "You are working (recent input) or the PC is under meaningful CPU/GPU load.",
  idle: "PC is on but there is no recent input and load is low.",
  sleep:
    "Sleep or Modern Standby. On many laptops Modern Standby is mostly the system running with the screen off, so it is priced by how long it was actually active, not as ~1 W.",
  hibernate: "Hibernated: almost no power is drawn.",
  off: "Shut down. Only the standby draw of the power supply and board.",
};

/** Round `max` up to a clean axis maximum and return evenly spaced ticks starting at 0. */
export function niceTicks(max: number, target = 4): number[] {
  if (!(max > 0)) return [0, 1];
  const rough = max / target;
  const pow = 10 ** Math.floor(Math.log10(rough));
  const f = rough / pow;
  const step = (f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10) * pow;
  const top = Math.ceil(max / step) * step;
  const out: number[] = [];
  for (let v = 0; v <= top + step / 2; v += step) out.push(Number(v.toPrecision(12)));
  return out;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Axis label for a bucket from its local ISO start ("2026-11-06T14:00:00"). */
export function bucketLabel(localStart: string, bucket: "hour" | "day" | "month"): string {
  const [date, time = "00:00:00"] = localStart.split("T");
  const [y, m, d] = date.split("-").map(Number);
  if (bucket === "hour") return time.slice(0, 2);
  if (bucket === "month") return MONTHS[m - 1];
  return `${d} ${MONTHS[m - 1]}${y ? "" : ""}`;
}

export function bucketTitle(localStart: string, bucket: "hour" | "day" | "month"): string {
  const [date, time = "00:00:00"] = localStart.split("T");
  const [y, m, d] = date.split("-").map(Number);
  if (bucket === "hour") return `${d} ${MONTHS[m - 1]}, ${time.slice(0, 5)}`;
  if (bucket === "month") return `${MONTHS[m - 1]} ${y}`;
  return `${d} ${MONTHS[m - 1]} ${y}`;
}

/** 3725 -> "01:02:05" (a stopwatch face). */
export function clock(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(Math.floor(s / 3600))}:${p(Math.floor((s % 3600) / 60))}:${p(s % 60)}`;
}

/** "1 h 02 min", "4 min 10 s", "35 s": a readable duration for results. */
export function duration(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min ${String(s % 60).padStart(2, "0")} s`;
  return `${Math.floor(s / 3600)} h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")} min`;
}

/** Short measurements are tiny: below 0.1 kWh show watt-hours ("8.5 Wh"), otherwise kWh. */
export function energy(nano: number): string {
  const v = nano / NANO;
  if (v >= 0.1) return kwh(nano);
  const wh = v * 1000;
  const digits = wh >= 10 ? 1 : 2;
  return `${wh.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits })} Wh`;
}

function localDate(iso: string): Date {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d);
}

/** Heading for a period: "Friday, 6 November 2026", "26 Oct - 1 Nov 2026", "November 2026", "2026". */
export function periodTitle(kind: "today" | "week" | "month" | "year", startIso: string, endIso: string, locale?: string): string {
  const s = localDate(startIso);
  const e = localDate(endIso);
  const f = (o: Intl.DateTimeFormatOptions, d: Date) => new Intl.DateTimeFormat(locale, o).format(d);
  switch (kind) {
    case "today":
      return f({ weekday: "long", day: "numeric", month: "long", year: "numeric" }, s);
    case "week": {
      const sameYear = s.getFullYear() === e.getFullYear();
      const left = f(sameYear ? { day: "numeric", month: "short" } : { day: "numeric", month: "short", year: "numeric" }, s);
      return `${left} - ${f({ day: "numeric", month: "short", year: "numeric" }, e)}`;
    }
    case "month":
      return f({ month: "long", year: "numeric" }, s);
    case "year":
      return f({ year: "numeric" }, s);
  }
}

/** Words for the buttons: "previous day", "next week", ... */
export const PERIOD_UNIT: Record<"today" | "week" | "month" | "year", string> = { today: "day", week: "week", month: "month", year: "year" };