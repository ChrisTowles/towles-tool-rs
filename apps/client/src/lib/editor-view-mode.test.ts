import { describe, expect, it } from "vitest";
import { modeForPanels, panelsFor, type EditorViewMode } from "./editor-view-mode";

const MODES: EditorViewMode[] = ["code", "split", "preview"];

describe("panelsFor", () => {
  it("gives each mode the halves it names", () => {
    expect(panelsFor("code")).toEqual({ editor: true, preview: false });
    expect(panelsFor("split")).toEqual({ editor: true, preview: true });
    expect(panelsFor("preview")).toEqual({ editor: false, preview: true });
  });

  it("never closes both halves", () => {
    for (const mode of MODES) {
      const { editor, preview } = panelsFor(mode);
      expect(editor || preview).toBe(true);
    }
  });
});

describe("modeForPanels", () => {
  it("round-trips every mode", () => {
    for (const mode of MODES) {
      const { editor, preview } = panelsFor(mode);
      expect(modeForPanels(editor, preview)).toBe(mode);
    }
  });

  it("reads a both-collapsed layout as code rather than inventing a fourth mode", () => {
    expect(modeForPanels(false, false)).toBe("code");
  });
});
