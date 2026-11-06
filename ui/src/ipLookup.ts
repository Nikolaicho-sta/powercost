// OPTIONAL region lookup. It is a network call, so it only ever runs after the user ticks the
// opt-in box and presses the button. It sends nothing but the request itself (your IP address is
// visible to the service, as with any web request). Documented in docs/PRIVACY.md.

export const IP_LOOKUP_URL = "https://ipapi.co/json/";

export interface IpRegion {
  country: string; // ISO 3166-1 alpha-2
  timezone: string | null;
}

/** Defensive parser: the answer is untrusted input. Only a 2-letter country code is accepted. */
export function parseIpLookup(data: unknown): IpRegion | null {
  if (typeof data !== "object" || data === null) return null;
  const o = data as Record<string, unknown>;
  const code = typeof o.country_code === "string" ? o.country_code.trim().toUpperCase() : "";
  if (!/^[A-Z]{2}$/.test(code)) return null;
  const tz = typeof o.timezone === "string" && /^[A-Za-z_]+(\/[A-Za-z0-9_+-]+)+$/.test(o.timezone) ? o.timezone : null;
  return { country: code, timezone: tz };
}

export async function lookupRegion(fetchFn: typeof fetch = fetch): Promise<IpRegion> {
  const res = await fetchFn(IP_LOOKUP_URL, { headers: { Accept: "application/json" } });
  if (!res.ok) throw new Error(`lookup failed (${res.status})`);
  const parsed = parseIpLookup(await res.json());
  if (!parsed) throw new Error("the lookup service returned an answer we could not use");
  return parsed;
}