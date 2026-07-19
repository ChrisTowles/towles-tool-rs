import { describe, expect, it } from "vitest";
import { isKnownBenignEntry } from "./wdio-console";

describe("isKnownBenignEntry", () => {
  it("excludes the wdio plugin's two unconditional boot warnings", () => {
    expect(
      isKnownBenignEntry(
        "warn",
        "[WDIO Tauri Plugin] TEST: This is a test WARN log after setupConsoleForwarding()",
      ),
    ).toBe(true);
    expect(
      isKnownBenignEntry(
        "warn",
        "[WDIO Tauri Plugin] ⚠️ Invoke interception via defineProperty failed; mock routing via window.__wdio_mocks__ remains active",
      ),
    ).toBe(true);
  });

  it("excludes tauri core's unlisten-race rejection (tauri-apps/tauri#8916)", () => {
    expect(
      isKnownBenignEntry(
        "rejection",
        "TypeError: undefined is not an object (evaluating 'listeners[eventId].handlerId')",
      ),
    ).toBe(true);
  });

  it("keeps app-side entries that merely mention the benign fragments", () => {
    // An app error that quotes the tauri fragment is OUR failure, not the
    // injected script's — kind and anchoring both have to match.
    expect(
      isKnownBenignEntry(
        "error",
        "cleanup failed TypeError: undefined is not an object (evaluating 'listeners[eventId].handlerId')",
      ),
    ).toBe(false);
    expect(
      isKnownBenignEntry(
        "rejection",
        "wrapped: TypeError: undefined is not an object (evaluating 'listeners[eventId].handlerId')",
      ),
    ).toBe(false);
    expect(
      isKnownBenignEntry("error", "TEST: This is a test WARN log after setupConsoleForwarding()"),
    ).toBe(false);
  });

  it("keeps every other line — including other plugin-prefixed failures", () => {
    expect(
      isKnownBenignEntry("warn", "[WDIO Tauri Plugin] failed to attach backend log listener"),
    ).toBe(false);
    expect(
      isKnownBenignEntry("warn", "Warning: validateDOMNesting(...): <button> cannot appear"),
    ).toBe(false);
    expect(isKnownBenignEntry("rejection", "")).toBe(false);
  });
});
