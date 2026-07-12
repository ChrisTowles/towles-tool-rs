import { describe, expect, it } from "vitest";
import {
  decodeTaskDrag,
  encodeTaskDrag,
  isTaskDrag,
  reorderedPosition,
  TASK_DRAG_TYPE,
  taskDropAction,
} from "./kanban-dnd";

describe("encodeTaskDrag / decodeTaskDrag", () => {
  it("round-trips a payload", () => {
    const encoded = encodeTaskDrag({ id: 42, status: "doing" });
    expect(decodeTaskDrag(encoded)).toEqual({ id: 42, status: "doing" });
  });

  it("rejects non-JSON", () => {
    expect(decodeTaskDrag("")).toBeNull();
    expect(decodeTaskDrag("not json")).toBeNull();
  });

  it("rejects JSON that is not a payload object", () => {
    expect(decodeTaskDrag("42")).toBeNull();
    expect(decodeTaskDrag("null")).toBeNull();
    expect(decodeTaskDrag('["doing"]')).toBeNull();
  });

  it("rejects a missing or non-integer id", () => {
    expect(decodeTaskDrag('{"status":"doing"}')).toBeNull();
    expect(decodeTaskDrag('{"id":"7","status":"doing"}')).toBeNull();
    expect(decodeTaskDrag('{"id":1.5,"status":"doing"}')).toBeNull();
  });

  it("rejects an unknown or missing status", () => {
    expect(decodeTaskDrag('{"id":1}')).toBeNull();
    expect(decodeTaskDrag('{"id":1,"status":"bogus"}')).toBeNull();
  });
});

describe("isTaskDrag", () => {
  it("matches only drags carrying the task type", () => {
    expect(isTaskDrag([TASK_DRAG_TYPE])).toBe(true);
    expect(isTaskDrag(["text/plain", TASK_DRAG_TYPE])).toBe(true);
    expect(isTaskDrag(["text/plain", "text/uri-list"])).toBe(false);
    expect(isTaskDrag([])).toBe(false);
  });
});

describe("taskDropAction", () => {
  it("moves a card dropped on another column", () => {
    const data = encodeTaskDrag({ id: 7, status: "backlog" });
    expect(taskDropAction(data, "review")).toEqual({ id: 7, status: "review" });
  });

  it("is a no-op when dropped back on its own column", () => {
    const data = encodeTaskDrag({ id: 7, status: "doing" });
    expect(taskDropAction(data, "doing")).toBeNull();
  });

  it("is a no-op for a foreign/garbage payload", () => {
    expect(taskDropAction("", "doing")).toBeNull();
    expect(taskDropAction("https://example.com", "doing")).toBeNull();
  });
});

describe("reorderedPosition", () => {
  it("lands at 0 in an empty column", () => {
    expect(reorderedPosition(null, null)).toBe(0);
  });

  it("goes just before the first card at the top", () => {
    expect(reorderedPosition(null, 0)).toBe(-1);
  });

  it("goes just after the last card at the bottom", () => {
    expect(reorderedPosition(4, null)).toBe(5);
  });

  it("splits the gap between two neighbors and stays between them", () => {
    const mid = reorderedPosition(1, 2);
    expect(mid).toBeGreaterThan(1);
    expect(mid).toBeLessThan(2);
  });
});
