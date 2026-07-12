import { describe, expect, it } from "vitest";
import { currentOrNextEvent, eventIsLive, type CalEvent } from "./data";

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
