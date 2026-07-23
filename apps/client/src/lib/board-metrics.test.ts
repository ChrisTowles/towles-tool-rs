import { describe, expect, it } from "vitest";
import { countByStatus } from "./board-metrics";
import type { TaskItem } from "./data";

function task(partial: Pick<TaskItem, "status"> & Partial<TaskItem>): TaskItem {
  return {
    id: 1,
    text: "t",
    position: 0,
    createdAt: 0,
    issues: [],
    prs: [],
    closed: partial.status === "done" || partial.outcome !== undefined,
    hasWorktree: partial.worktree?.dir !== undefined,
    ...partial,
  };
}

describe("countByStatus", () => {
  it("counts cards per column and zero-fills empty ones", () => {
    const counts = countByStatus([
      task({ status: "backlog" }),
      task({ status: "backlog" }),
      task({ status: "doing" }),
    ]);
    expect(counts).toEqual({ backlog: 2, doing: 1, done: 0 });
  });
});
