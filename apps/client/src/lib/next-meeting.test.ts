import { describe, expect, it } from "vitest";
import { currentOrNextEvent, eventIsLive, fmtCountdown, type CalEvent } from "./data";

const ev = (id: number, startTs: number, endTs?: number): CalEvent => ({
  id,
  externalId: `e${id}`,
  title: `Event ${id}`,
  startTs,
  endTs,
  attendees: [],
});

describe("currentOrNextEvent", () => {
  // "b" runs [300, 1300); "c" runs [1500, 2500).
  const events = [ev(2, 300, 1300), ev(3, 1500, 2500)];

  it("returns the next future meeting before it starts", () => {
    expect(currentOrNextEvent(events, 200)?.id).toBe(2);
  });

  it("keeps an in-progress meeting selected instead of skipping ahead", () => {
    expect(currentOrNextEvent(events, 300)?.id).toBe(2); // exact start = live
    expect(currentOrNextEvent(events, 800)?.id).toBe(2);
  });

  it("moves to the next meeting once the live one ends", () => {
    expect(currentOrNextEvent(events, 1300)?.id).toBe(3);
  });

  it("returns undefined once every meeting has ended", () => {
    expect(currentOrNextEvent(events, 3000)).toBeUndefined();
  });

  it("treats an event with no endTs as a point in time (shown up to its start)", () => {
    const open = [ev(9, 500)];
    expect(currentOrNextEvent(open, 400)?.id).toBe(9);
    expect(currentOrNextEvent(open, 500)?.id).toBe(9);
    expect(currentOrNextEvent(open, 600)).toBeUndefined();
  });
});

describe("eventIsLive", () => {
  it("is true only within [startTs, endTs)", () => {
    const e = ev(1, 300, 1300);
    expect(eventIsLive(e, 200)).toBe(false);
    expect(eventIsLive(e, 300)).toBe(true);
    expect(eventIsLive(e, 1299)).toBe(true);
    expect(eventIsLive(e, 1300)).toBe(false);
  });

  it("is never live without an endTs", () => {
    expect(eventIsLive(ev(1, 300), 400)).toBe(false);
  });
});

describe("fmtCountdown", () => {
  it("is `now` at zero or in the past", () => {
    expect(fmtCountdown(0)).toBe("now");
    expect(fmtCountdown(-5000)).toBe("now");
  });

  it("uses minute/hour granularity above the ~2m threshold (unchanged)", () => {
    expect(fmtCountdown(120_000)).toBe("2m"); // exactly at the threshold
    expect(fmtCountdown(22 * 60_000)).toBe("22m");
    expect(fmtCountdown(65 * 60_000)).toBe("1h 05m");
  });

  it("switches to m:ss under the threshold", () => {
    expect(fmtCountdown(119_000)).toBe("1:59");
    expect(fmtCountdown(90_000)).toBe("1:30");
    expect(fmtCountdown(59_000)).toBe("0:59");
    expect(fmtCountdown(5_000)).toBe("0:05");
  });

  it("rounds a partial second up so it never shows 0:00 while positive", () => {
    expect(fmtCountdown(500)).toBe("0:01");
  });
});
