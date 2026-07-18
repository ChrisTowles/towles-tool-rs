import { describe, expect, it } from "vitest";
import { PRUNED_COMMANDS, staleCommands } from "@/lib/monaco-prune";

describe("staleCommands", () => {
  it("is empty when every shadowed id is still registered upstream", () => {
    expect(staleCommands([...PRUNED_COMMANDS, "some.other.command"])).toEqual([]);
  });

  // The failure this guards: a renamed id shadows nothing, the real handler
  // stays live, and the hazard silently returns.
  it("names the ids that no longer exist upstream", () => {
    const live = PRUNED_COMMANDS.filter((id) => id !== "deleteFile");
    expect(staleCommands(live)).toEqual(["deleteFile"]);
  });

  it("reports every missing id, not just the first", () => {
    expect(staleCommands([])).toEqual([...PRUNED_COMMANDS]);
  });
});

describe("PRUNED_COMMANDS", () => {
  // The window.confirm path this whole module exists to close: the format
  // "none" action prompts to install a formatter, and the file actions
  // confirm before deleting.
  it("covers both commands that reach IDialogService.confirm", () => {
    expect(PRUNED_COMMANDS).toContain("editor.action.formatDocument.none");
    expect(PRUNED_COMMANDS).toContain("deleteFile");
  });

  // Delete has no writable precondition, and with no trash capability the
  // plain Delete key maps to moveFileToTrash — shadowing one without the
  // other leaves the keystroke live.
  it("covers moveFileToTrash alongside deleteFile", () => {
    expect(PRUNED_COMMANDS).toContain("moveFileToTrash");
  });

  it("has no duplicate ids (a duplicate would shadow our own no-op)", () => {
    expect(new Set(PRUNED_COMMANDS).size).toBe(PRUNED_COMMANDS.length);
  });
});
