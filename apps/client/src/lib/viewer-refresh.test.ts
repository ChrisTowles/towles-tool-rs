import { describe, expect, it } from "vitest";
import { diskChangeAction } from "./viewer-refresh";

describe("diskChangeAction", () => {
  it("ignores the echo of the viewer's own save (mtime already known)", () => {
    expect(diskChangeAction({ dirty: false, bufferMtimeMs: 100, diskMtimeMs: 100 })).toBe("ignore");
    // Typed-during-save: buffer is dirty but disk is exactly what we wrote.
    expect(diskChangeAction({ dirty: true, bufferMtimeMs: 100, diskMtimeMs: 100 })).toBe("ignore");
  });

  it("reloads a clean buffer when the disk moved on", () => {
    expect(diskChangeAction({ dirty: false, bufferMtimeMs: 100, diskMtimeMs: 200 })).toBe("reload");
  });

  it("raises a conflict instead of clobbering unsaved edits", () => {
    expect(diskChangeAction({ dirty: true, bufferMtimeMs: 100, diskMtimeMs: 200 })).toBe(
      "conflict",
    );
  });

  it("treats an unknown buffer mtime as a real change", () => {
    expect(diskChangeAction({ dirty: false, bufferMtimeMs: null, diskMtimeMs: 200 })).toBe(
      "reload",
    );
    expect(diskChangeAction({ dirty: true, bufferMtimeMs: null, diskMtimeMs: 200 })).toBe(
      "conflict",
    );
  });
});
