import { describe, expect, it } from "vitest";
import { IpcFailed, IpcTimeout, NotInTauri, SchemaMismatch, errorMessage } from "./errors";

describe("errorMessage", () => {
  it("passes a bare string through — Tauri rejects commands with one", () => {
    expect(errorMessage("no such repo")).toBe("no such repo");
  });

  it("reads an Error's message rather than its 'Error: ' prefix", () => {
    expect(errorMessage(new Error("boom"))).toBe("boom");
  });

  it("serializes a plain object instead of yielding [object Object]", () => {
    expect(errorMessage({ code: 42 })).toBe('{"code":42}');
  });

  it("survives a value JSON.stringify rejects", () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(errorMessage(cyclic)).toBe("[object Object]");
  });

  it("survives a BigInt, which JSON.stringify throws on", () => {
    expect(errorMessage(10n)).toBe("10");
  });

  it("names the absent case rather than printing null/undefined", () => {
    expect(errorMessage(null)).toBe("unknown error");
    expect(errorMessage(undefined)).toBe("unknown error");
  });

  it("prefers a tagged error's composed message", () => {
    expect(errorMessage(new NotInTauri({ command: "settings_get" }))).toBe(
      "not running under Tauri (settings_get)",
    );
  });
});

describe("IpcError variants", () => {
  it("composes IpcFailed from the command and its cause", () => {
    const error = new IpcFailed({ command: "store_add_task", cause: "db locked" });
    expect(error.message).toBe("store_add_task: db locked");
    expect(error.command).toBe("store_add_task");
    expect(error.cause).toBe("db locked");
  });

  it("names the timeout budget so a stuck command is identifiable", () => {
    expect(new IpcTimeout({ command: "ab_sync_repo", timeoutMs: 5000 }).message).toBe(
      "ab_sync_repo: timed out after 5000ms",
    );
  });

  it("summarizes every schema issue with its path", () => {
    const error = new SchemaMismatch({
      command: "settings_get",
      issues: [
        { code: "invalid_type", path: ["collectors", "prs"], message: "expected object" },
        { code: "invalid_type", path: [], message: "expected object" },
      ] as SchemaMismatch["issues"],
    });
    expect(error.message).toBe(
      "settings_get: response failed validation — collectors.prs: expected object; (root): expected object",
    );
  });

  it("discriminates variants by class guard, not by tag comparison", () => {
    const notInTauri: unknown = new NotInTauri({ command: "app_slot" });
    expect(NotInTauri.is(notInTauri)).toBe(true);
    expect(IpcFailed.is(notInTauri)).toBe(false);
  });
});
