import { describe, expect, it } from "vitest";
import { dmsNeedingAttention, EMPTY_SNAPSHOT, type DmItem, type StoreSnapshot } from "./data";

function dm(overrides: Partial<DmItem> = {}): DmItem {
  return {
    channel: "D123",
    fromName: "Wife",
    text: "call me",
    ts: 1000,
    fromMe: false,
    fetchedAt: 1000,
    dismissedTs: 0,
    ...overrides,
  };
}

function snapshotWith(dms: DmItem[]): StoreSnapshot {
  return { ...EMPTY_SNAPSHOT, dms };
}

describe("dmsNeedingAttention", () => {
  it("counts an unanswered DM (theirs, undismissed)", () => {
    expect(dmsNeedingAttention(snapshotWith([dm()]))).toHaveLength(1);
  });

  it("ignores a DM whose newest message is mine (answered)", () => {
    expect(dmsNeedingAttention(snapshotWith([dm({ fromMe: true })]))).toHaveLength(0);
  });

  it("ignores a DM dismissed at or after its newest message", () => {
    expect(dmsNeedingAttention(snapshotWith([dm({ ts: 1000, dismissedTs: 1000 })]))).toHaveLength(
      0,
    );
    expect(dmsNeedingAttention(snapshotWith([dm({ ts: 1000, dismissedTs: 1500 })]))).toHaveLength(
      0,
    );
  });

  it("re-surfaces a DM with a newer message after an earlier dismiss", () => {
    expect(dmsNeedingAttention(snapshotWith([dm({ ts: 2000, dismissedTs: 1000 })]))).toHaveLength(
      1,
    );
  });

  it("returns only the DMs that need attention from a mixed set", () => {
    const needing = dmsNeedingAttention(
      snapshotWith([
        dm({ channel: "A" }),
        dm({ channel: "B", fromMe: true }),
        dm({ channel: "C", ts: 1000, dismissedTs: 1000 }),
        dm({ channel: "D", ts: 3000, dismissedTs: 1000 }),
      ]),
    );
    expect(needing.map((d) => d.channel)).toEqual(["A", "D"]);
  });
});
