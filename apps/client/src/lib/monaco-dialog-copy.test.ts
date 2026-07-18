import { describe, expect, it } from "vitest";
import { deleteCopyForTrash, isDangerous, stripMnemonic } from "@/lib/monaco-dialog-copy";

describe("stripMnemonic", () => {
  it("drops VS Code's && mnemonic markers", () => {
    expect(stripMnemonic("&&Delete")).toBe("Delete");
    expect(stripMnemonic("Move to &&Trash")).toBe("Move to Trash");
  });

  it("leaves a plain label alone", () => {
    expect(stripMnemonic("OK")).toBe("OK");
  });
});

describe("deleteCopyForTrash", () => {
  // VS Code asks to "permanently delete" because the overlay hides our Trash
  // capability — but the provider trashes, so the wording has to be corrected
  // or the dialog lies about what it is about to do.
  it("rewrites the permanent-delete wording and promises a restore", () => {
    const out = deleteCopyForTrash(
      "Are you sure you want to permanently delete 'notes.txt'?",
      "This action is irreversible!",
    );
    expect(out.message).toBe("Are you sure you want to delete 'notes.txt'?");
    expect(out.detail).toBe("You can restore it from your system Trash.");
  });

  it("replaces the irreversible warning rather than keeping it", () => {
    const out = deleteCopyForTrash("permanently delete x", "This action is irreversible!");
    expect(out.detail).not.toMatch(/irreversible/i);
  });

  it("passes an unrelated confirmation through untouched", () => {
    const out = deleteCopyForTrash("Save changes to app.ts?", "Your edits will be lost.");
    expect(out).toEqual({ message: "Save changes to app.ts?", detail: "Your edits will be lost." });
  });
});

describe("isDangerous", () => {
  it("flags destructive verbs so the confirm gets the destructive button", () => {
    expect(isDangerous("Delete", "Are you sure you want to delete 'a.txt'?")).toBe(true);
    expect(isDangerous("Move to Trash", "Delete a.txt?")).toBe(true);
    expect(isDangerous("OK", "Discard your changes?")).toBe(true);
  });

  it("leaves an ordinary confirmation alone", () => {
    expect(isDangerous("Save", "Save changes to app.ts?")).toBe(false);
  });
});
