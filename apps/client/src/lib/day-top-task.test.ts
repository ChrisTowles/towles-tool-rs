import { describe, expect, it } from "vitest";
import type { TaskItem, TaskStatus } from "@/lib/data";
import { pickTopTask } from "@/lib/day-top-task";

let nextId = 1;

function task(status: TaskStatus, over: Partial<TaskItem> = {}): TaskItem {
  return {
    id: nextId++,
    text: `${status}-task`,
    status,
    position: 0,
    createdAt: 0,
    issues: [],
    prs: [],
    ...over,
  };
}

describe("pickTopTask", () => {
  it("returns undefined when there are no tasks", () => {
    expect(pickTopTask([])).toBeUndefined();
  });

  it("returns undefined when every task is done", () => {
    expect(pickTopTask([task("done"), task("done")])).toBeUndefined();
  });

  it("prefers in-progress work over an older backlog item", () => {
    const backlog = task("backlog", { createdAt: 1 });
    const doing = task("doing", { createdAt: 1_000_000 });
    expect(pickTopTask([backlog, doing])).toBe(doing);
  });

  it("orders doing > backlog", () => {
    const backlog = task("backlog");
    const doing = task("doing");
    expect(pickTopTask([backlog, doing])).toBe(doing);
    expect(pickTopTask([backlog])).toBe(backlog);
  });

  it("never surfaces a done task even if it is the only in-progress-looking one", () => {
    const done = task("done");
    const backlog = task("backlog");
    expect(pickTopTask([done, backlog])).toBe(backlog);
  });

  it("never surfaces a closed task, even one abandoned mid-doing", () => {
    // Abandoned tasks freeze their status where the work stopped — without
    // the closed check, this "doing" card would outrank every live one.
    const abandoned = task("doing", { outcome: "abandoned" });
    const backlog = task("backlog");
    expect(pickTopTask([abandoned, backlog])).toBe(backlog);
    expect(pickTopTask([abandoned])).toBeUndefined();
  });

  it("breaks status ties by column position (nearer the top wins)", () => {
    const lower = task("doing", { position: 5 });
    const higher = task("doing", { position: 1 });
    expect(pickTopTask([lower, higher])).toBe(higher);
  });

  it("ignores createdAt within the same status once position differs", () => {
    const oldButLower = task("doing", { position: 10, createdAt: 1 });
    const newButHigher = task("doing", { position: 2, createdAt: 9_999 });
    expect(pickTopTask([oldButLower, newButHigher])).toBe(newButHigher);
  });
});
