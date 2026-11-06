import { describe, expect, it } from "vitest";
import { bucketLabel, bucketTitle, clock, duration, energy, hours, kwh, niceTicks, percent, share } from "./format";

describe("niceTicks", () => {
  it("rounds up to clean steps starting at zero", () => {
    expect(niceTicks(0.83)).toEqual([0, 0.5, 1]);
    expect(niceTicks(7)).toEqual([0, 2, 4, 6, 8]);
    expect(niceTicks(130)).toEqual([0, 50, 100, 150]);
  });
  it("copes with empty data", () => {
    expect(niceTicks(0)).toEqual([0, 1]);
  });
});

describe("formatting", () => {
  it("formats energy and time", () => {
    expect(kwh(1_234_000_000)).toMatch(/1[.,]23/); // decimal separator follows the locale
    expect(hours(5400)).toBe("1.5 h");
    expect(percent(965)).toBe("96.5%");
    expect(percent(500)).toBe("50%");
  });
  it("computes shares in per-mille", () => {
    const a = { seconds: 0, kwh_nano: 0, cost_nano: 250, cost_minor: 0 };
    const w = { seconds: 0, kwh_nano: 0, cost_nano: 1000, cost_minor: 0 };
    expect(share(a, w)).toBe(250);
    expect(share(a, { ...w, cost_nano: 0 })).toBe(0);
  });
  it("labels buckets from local ISO starts", () => {
    expect(bucketLabel("2026-11-06T14:00:00", "hour")).toBe("14");
    expect(bucketLabel("2026-11-06T00:00:00", "day")).toBe("6 Nov");
    expect(bucketLabel("2026-03-01T00:00:00", "month")).toBe("Mar");
    expect(bucketTitle("2026-11-06T14:00:00", "hour")).toBe("6 Nov, 14:00");
  });
});

describe("stopwatch formatting", () => {
  it("formats the clock face", () => {
    expect(clock(0)).toBe("00:00:00");
    expect(clock(65)).toBe("00:01:05");
    expect(clock(3725.9)).toBe("01:02:05");
    expect(clock(-5)).toBe("00:00:00");
    expect(clock(100 * 3600)).toBe("100:00:00");
  });
  it("formats readable durations", () => {
    expect(duration(35)).toBe("35 s");
    expect(duration(250)).toBe("4 min 10 s");
    expect(duration(3720)).toBe("1 h 02 min");
    expect(duration(0)).toBe("0 s");
  });
});

describe("energy for short measurements", () => {
  it("switches to watt-hours below 0.1 kWh", () => {
    expect(energy(8_500_000)).toMatch(/8[.,]50 Wh/);
    expect(energy(45_000_000)).toMatch(/45[.,]0 Wh/);
    expect(energy(0)).toMatch(/0[.,]00 Wh/);
    expect(energy(1_234_000_000)).toMatch(/1[.,]23 kWh/);
  });
});