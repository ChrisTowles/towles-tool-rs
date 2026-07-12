import { describe, expect, it } from "vitest";
import { deadPaneAction } from "./term-protocol";

describe("deadPaneAction", () => {
  it("labels an exited pane as a restart", () => {
    expect(deadPaneAction({ hasSession: true, hasDir: true, exited: true })).toEqual({
      canRestart: true,
      label: "Restart shell",
    });
  });

  it("labels a never-started pane as a start", () => {
    expect(deadPaneAction({ hasSession: true, hasDir: true, exited: false })).toEqual({
      canRestart: true,
      label: "Start shell",
    });
  });

  it("cannot restart without a session record", () => {
    expect(deadPaneAction({ hasSession: false, hasDir: true, exited: true }).canRestart).toBe(false);
  });

  it("cannot restart without a folder dir to spawn in", () => {
    expect(deadPaneAction({ hasSession: true, hasDir: false, exited: true }).canRestart).toBe(false);
  });
});
