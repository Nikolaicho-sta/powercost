import { describe, expect, it } from "vitest";
import { formatMinor, parseMinor } from "./money";
import { lookupRegion, parseIpLookup } from "./ipLookup";
import { addPeriod, blankTariff, copyAsUser, isDecimalString, nextVersion, overnightRanges, parseHolidays, rangesCoverDay, removePeriod, renamePeriod, setComponentMode, slugify, type TariffDoc, dot } from "./tariffModel";

describe("money text", () => {
  it("parses exactly, without floats", () => {
    expect(parseMinor("12.50", 2)).toBe(1250);
    expect(parseMinor("12,5", 2)).toBe(1250);
    expect(parseMinor("0.07", 2)).toBe(7);
    expect(parseMinor("10", 2)).toBe(1000);
    expect(parseMinor(" 3 ", 0)).toBe(3);
  });
  it("rejects what it would have to round or cannot read", () => {
    expect(parseMinor("1.234", 2)).toBeNull();
    expect(parseMinor("abc", 2)).toBeNull();
    expect(parseMinor("-5", 2)).toBeNull();
    expect(parseMinor("", 2)).toBeNull();
    expect(parseMinor("99999999999999999999", 2)).toBeNull();
  });
  it("formats minor units", () => {
    expect(formatMinor(1250, 2)).toBe("12.50");
    expect(formatMinor(7, 2)).toBe("0.07");
    expect(formatMinor(5, 0)).toBe("5");
  });
});

describe("IP lookup parsing (untrusted input)", () => {
  it("accepts a normal answer", () => {
    expect(parseIpLookup({ country_code: "bg", timezone: "Europe/Sofia", ip: "1.2.3.4" })).toEqual({ country: "BG", timezone: "Europe/Sofia" });
  });
  it("rejects garbage and drops odd timezones", () => {
    expect(parseIpLookup(null)).toBeNull();
    expect(parseIpLookup({ country_code: "Bulgaria" })).toBeNull();
    expect(parseIpLookup({ country_code: "BG", timezone: "<script>" })).toEqual({ country: "BG", timezone: null });
    expect(parseIpLookup({})).toBeNull();
  });
  it("lookupRegion uses the injected fetch and reports failures", async () => {
    const ok = (async () => ({ ok: true, status: 200, json: async () => ({ country_code: "DE", timezone: "Europe/Berlin" }) })) as unknown as typeof fetch;
    await expect(lookupRegion(ok)).resolves.toEqual({ country: "DE", timezone: "Europe/Berlin" });
    const bad = (async () => ({ ok: false, status: 429, json: async () => ({}) })) as unknown as typeof fetch;
    await expect(lookupRegion(bad)).rejects.toThrow(/429/);
    const junk = (async () => ({ ok: true, status: 200, json: async () => ({ hello: 1 }) })) as unknown as typeof fetch;
    await expect(lookupRegion(junk)).rejects.toThrow(/could not use/);
  });
});

describe("tariff helpers", () => {
  it("slugifies names into valid ids", () => {
    expect(slugify("My Home Tariff (2026)!")).toBe("my-home-tariff-2026");
    expect(slugify("***")).toBe("tariff");
  });
  it("copies a preset as the user's own and versions it", () => {
    const base = blankTariff("BG", "EUR", "Europe/Sofia", "2026-11-06");
    base.id = "bg-electrohold";
    const mine = copyAsUser(base, "2026-11-06");
    expect(mine.id).toBe("user-bg-electrohold");
    expect(mine.verification).toBe("unverified");
    expect(base.id).toBe("bg-electrohold"); // original untouched
    const v2 = nextVersion(mine, "2027-07-01");
    expect([v2.id, v2.version, v2.valid_from]).toEqual(["user-bg-electrohold", 2, "2027-07-01"]);
  });
  it("splits overnight windows at midnight", () => {
    expect(overnightRanges("22:00", "06:00")).toEqual([{ start: "00:00", end: "06:00" }, { start: "22:00", end: "24:00" }]);
    expect(overnightRanges("07:00", "23:00")).toEqual([{ start: "07:00", end: "23:00" }]);
    expect(overnightRanges("00:00", "00:00")).toEqual([{ start: "00:00", end: "24:00" }]);
    expect(overnightRanges("22:00", "00:00")).toEqual([{ start: "22:00", end: "24:00" }]);
  });
  it("detects whether ranges tile the day", () => {
    expect(rangesCoverDay([{ start: "00:00", end: "06:00" }, { start: "06:00", end: "24:00" }])).toBe(true);
    expect(rangesCoverDay([{ start: "00:00", end: "06:00" }, { start: "07:00", end: "24:00" }])).toBe(false);
    expect(rangesCoverDay([{ start: "00:00", end: "12:00" }])).toBe(false);
    expect(rangesCoverDay([{ start: "00:00", end: "24:00" }, { start: "05:00", end: "06:00" }])).toBe(false);
  });
  it("recognises decimal strings", () => {
    expect(["0.1", "12", "-3.5"].every(isDecimalString)).toBe(true);
    expect(["", "1e3", "abc", "1,5"].some(isDecimalString)).toBe(false);
  });
});

function twoZone(): TariffDoc {
  const t = blankTariff("BG", "EUR", "Europe/Sofia", "2026-01-01");
  t.periods = [{ id: "day", label: "Day" }, { id: "night", label: "Night" }];
  t.components = [{ id: "e", kind: "energy", vat_inclusive: false, per_period: { day: "0.2", night: "0.1" } }, { id: "n", kind: "network", vat_inclusive: false, per_kwh: "0.03" }];
  t.schedule.seasons = [{ id: "all", from: "01-01", to: "12-31", rules: [{ days: ["all"], ranges: [{ start: "00:00", end: "06:00" }], period: "night" }, { days: ["all"], ranges: [{ start: "06:00", end: "24:00" }], period: "day" }] }];
  return t;
}

describe("period editing keeps references consistent", () => {
  it("renames everywhere", () => {
    const r = renamePeriod(twoZone(), "night", "cheap");
    expect(r.periods.map((p) => p.id)).toEqual(["day", "cheap"]);
    expect(Object.keys(r.components[0].per_period!)).toEqual(["day", "cheap"]);
    expect(r.schedule.seasons[0].rules.map((x) => x.period)).toEqual(["cheap", "day"]);
  });
  it("removes a period with its prices and rules", () => {
    const r = removePeriod(twoZone(), "night");
    expect(r.periods.map((p) => p.id)).toEqual(["day"]);
    expect(r.components[0].per_period).toEqual({ day: "0.2" });
    expect(r.schedule.seasons[0].rules.map((x) => x.period)).toEqual(["day"]);
  });
  it("adds a period seeded with an existing price", () => {
    const r = addPeriod(twoZone(), "peak", "Peak");
    expect(r.components[0].per_period!.peak).toBe("0.2");
    expect(r.components[1].per_kwh).toBe("0.03"); // flat components are untouched
  });
  it("switches a component between flat and per-period without ever keeping both", () => {
    const t = twoZone();
    const flat = setComponentMode(t.components[0], "flat", t.periods);
    expect(flat.per_kwh).toBe("0.2");
    expect(flat.per_period).toBeUndefined();
    const per = setComponentMode(t.components[1], "per_period", t.periods);
    expect(per.per_period).toEqual({ day: "0.03", night: "0.03" });
    expect(per.per_kwh).toBeUndefined();
  });
  it("does not mutate its input", () => {
    const t = twoZone();
    const before = JSON.stringify(t);
    renamePeriod(t, "day", "x");
    removePeriod(t, "day");
    addPeriod(t, "p", "P");
    expect(JSON.stringify(t)).toBe(before);
  });
});

describe("holiday parsing", () => {
  it("splits, sorts, dedupes and reports bad entries", () => {
    expect(parseHolidays("2026-12-25, 2026-01-01\n2026-01-01; 25/12/2026")).toEqual({ dates: ["2026-01-01", "2026-12-25"], bad: ["25/12/2026"] });
    expect(parseHolidays("")).toEqual({ dates: [], bad: [] });
  });
});

describe("decimal comma", () => {
  it("is normalised to a dot for the data format", () => {
    expect(dot("24,5")).toBe("24.5");
    expect(dot("0.12")).toBe("0.12");
    expect(isDecimalString(dot("1,5"))).toBe(true);
  });
});
