// Money text <-> integer minor units WITHOUT floats: "12.50" -> 1250. Used for the budget input.

/** Parse a user-typed decimal amount into minor units; null when it is not a valid amount. */
export function parseMinor(text: string, digits: number): number | null {
  const t = text.trim().replace(",", ".");
  if (!/^\d+(\.\d+)?$/.test(t)) return null;
  const [whole, frac = ""] = t.split(".");
  if (frac.length > digits) return null; // never silently round money
  const minor = Number(whole + frac.padEnd(digits, "0"));
  return Number.isSafeInteger(minor) ? minor : null;
}

export function formatMinor(minor: number, digits: number): string {
  if (digits === 0) return String(minor);
  const unit = 10 ** digits;
  const whole = Math.trunc(minor / unit);
  const frac = String(Math.abs(minor % unit)).padStart(digits, "0");
  return `${whole}.${frac}`;
}