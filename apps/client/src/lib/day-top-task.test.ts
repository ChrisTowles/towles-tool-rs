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

  it("orders doing > review > next > backlog", () => {
    const backlog = task("backlog");
    const next = task("next");
    const review = task("review");
    const doing = task("doing");
    expect(pickTopTask([backlog, next, review, doing])).toBe(doing);
    expect(pickTopTask([backlog, next, review])).toBe(review);
    expect(pickTopTask([backlog, next])).toBe(next);
    expect(pickTopTask([backlog])).toBe(backlog);
  });

  it("never surfaces a done task even if it is the only in-progress-looking one", () => {
    const done = task("done");
    const backlog = task("backlog");
    expect(pickTopTask([done, backlog])).toBe(backlog);
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
