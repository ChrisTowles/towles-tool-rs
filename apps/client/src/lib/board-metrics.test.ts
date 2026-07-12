import { describe, expect, it } from "vitest";
import { countByStatus, dueState, overdueByStatus } from "./board-metrics";
import type { TaskItem } from "./data";

/** Noon on 2026-07-12, local — a stable "now" well away from either day edge. */
const NOON = new Date(2026, 6, 12, 12, 0, 0, 0).getTime();

describe("dueState", () => {
  it("returns none when there is no due date", () => {
    expect(dueState(null, NOON)).toBe("none");
    expect(dueState(undefined, NOON)).toBe("none");
  });

  it("is overdue strictly before now", () => {
    expect(dueState(NOON - 1, NOON)).toBe("overdue");
  });

  it("treats exactly-now as today, not overdue", () => {
    // `< now` is strict, so the due instant landing on `now` is still today.
    expect(dueState(NOON, NOON)).toBe("today");
  });

  it("is today for a due instant later in the same local day", () => {
    const endOfDay = new Date(2026, 6, 12, 23, 59, 59, 999).getTime();
    expect(dueState(endOfDay, NOON)).toBe("today");
  });

  it("is future for a due instant on a later day", () => {
    const tomorrow = new Date(2026, 6, 13, 0, 0, 0, 0).getTime();
    expect(dueState(tomorrow, NOON)).toBe("future");
  });

  it("flips a due-yesterday card to overdue just past local midnight", () => {
    const justPastMidnight = new Date(2026, 6, 12, 0, 0, 30, 0).getTime();
    const dueYesterdayEnd = new Date(2026, 6, 11, 23, 59, 59, 999).getTime();
    expect(dueState(dueYesterdayEnd, justPastMidnight)).toBe("overdue");
  });

  it("keeps a due-today card as today just past local midnight", () => {
    const justPastMidnight = new Date(2026, 6, 12, 0, 0, 30, 0).getTime();
    const dueTodayEnd = new Date(2026, 6, 12, 23, 59, 59, 999).getTime();
    expect(dueState(dueTodayEnd, justPastMidnight)).toBe("today");
  });
});

function task(partial: Pick<TaskItem, "status"> & Partial<TaskItem>): TaskItem {
  return { id: 1, text: "t", position: 0, createdAt: 0, ...partial };
}

describe("countByStatus", () => {
  it("counts cards per column and zero-fills empty ones", () => {
    const counts = countByStatus([
      task({ status: "backlog" }),
      task({ status: "backlog" }),
      task({ status: "doing" }),
    ]);
    expect(counts).toEqual({ backlog: 2, next: 0, doing: 1, review: 0, done: 0 });
  });
});

describe("overdueByStatus", () => {
  it("counts only overdue, non-done cards per column", () => {
    const past = NOON - 1;
    const future = NOON + 86_400_000;
    const counts = overdueByStatus(
      [
        task({ status: "backlog", dueTs: past }),
        task({ status: "backlog", dueTs: future }),
        task({ status: "backlog" }),
        task({ status: "doing", dueTs: past }),
        task({ status: "done", dueTs: past }), // done is never late
      ],
      NOON,
    );
    expect(counts).toEqual({ backlog: 1, next: 0, doing: 1, review: 0, done: 0 });
  });
});
