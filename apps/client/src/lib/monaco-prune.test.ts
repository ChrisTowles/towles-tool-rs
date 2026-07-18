import { describe, expect, it } from "vitest";
import { PRUNED_COMMANDS, staleCommands } from "@/lib/monaco-prune";

describe("staleCommands", () => {
  it("is empty when every shadowed id is still registered upstream", () => {
    expect(staleCommands([...PRUNED_COMMANDS, "some.other.command"])).toEqual([]);
  });

  // The failure this guards: a renamed id shadows nothing, the real handler
  // stays live, and the hazard silently returns.
  it("names the ids that no longer exist upstream", () => {
    const live = PRUNED_COMMANDS.filter((id) => id !== "editor.action.formatDocument");
    expect(staleCommands(live)).toEqual(["editor.action.formatDocument"]);
  });

  it("reports every missing id, not just the first", () => {
    expect(staleCommands([])).toEqual([...PRUNED_COMMANDS]);
  });
});

describe("PRUNED_COMMANDS", () => {
  // The window.confirm path this module exists to close: with no formatter
  // registered for any language, every format action funnels into the
  // "install a formatter?" prompt.
  it("covers the format action that raises the prompt", () => {
    expect(PRUNED_COMMANDS).toContain("editor.action.formatDocument.none");
  });

  // These are real now: the provider writes through ide_* commands and the
  // dialog service renders an in-app confirm, so shadowing them would break
  // the Explorer rather than protect it.
  it("does not shadow the delete commands", () => {
    expect(PRUNED_COMMANDS).not.toContain("deleteFile");
    expect(PRUNED_COMMANDS).not.toContain("moveFileToTrash");
  });

  it("has no duplicate ids (a duplicate would shadow our own no-op)", () => {
    expect(new Set(PRUNED_COMMANDS).size).toBe(PRUNED_COMMANDS.length);
  });
});
