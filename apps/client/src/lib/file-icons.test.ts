import { describe, expect, it } from "vitest";
import {
  File,
  FileCode,
  FlaskConical,
  Folder,
  FolderCode,
  FolderGit2,
  FolderOpen,
  Package,
} from "lucide-react";
import { fileIconSpec, folderIconSpec } from "./file-icons";

describe("fileIconSpec", () => {
  it("matches by extension on the basename, case-insensitively", () => {
    expect(fileIconSpec("crates/tt-vt/src/main.rs").Icon).toBe(FileCode);
    expect(fileIconSpec("crates/tt-vt/src/main.rs").className).toContain("orange");
    expect(fileIconSpec("README.MD").Icon).not.toBe(File);
  });

  it("prefers exact basenames over extensions", () => {
    const spec = fileIconSpec("apps/client/package.json");
    expect(spec.Icon).toBe(Package);
    expect(spec.className).toContain("red");
    expect(fileIconSpec("Cargo.toml").Icon).toBe(Package);
  });

  it("recognizes test files ahead of their script extension", () => {
    expect(fileIconSpec("src/lib/diff.test.ts").Icon).toBe(FlaskConical);
    expect(fileIconSpec("e2e/specs/diff.e2e.ts").Icon).toBe(FlaskConical);
    expect(fileIconSpec("src/lib/diff.ts").Icon).toBe(FileCode);
  });

  it("marks Claude-owned files violet", () => {
    expect(fileIconSpec("CLAUDE.md").className).toContain("violet");
  });

  it("handles env-style dotfiles and falls back to a dim generic file", () => {
    expect(fileIconSpec(".env.local").className).toContain("yellow");
    expect(fileIconSpec("Makefile")).toEqual({
      Icon: File,
      className: "text-muted-foreground/50",
    });
    expect(fileIconSpec(".zshrc").Icon).toBe(File);
  });
});

describe("folderIconSpec", () => {
  it("swaps plain folders between closed and open icons", () => {
    expect(folderIconSpec("screens", false).Icon).toBe(Folder);
    expect(folderIconSpec("screens", true).Icon).toBe(FolderOpen);
  });

  it("colors special folders and keys compacted chains off the last segment", () => {
    expect(folderIconSpec(".github", false).Icon).toBe(FolderGit2);
    expect(folderIconSpec("apps/client/src", false).Icon).toBe(FolderCode);
    expect(folderIconSpec(".claude", true).className).toContain("violet");
    expect(folderIconSpec("node_modules", false).className).toContain("/40");
  });
});
