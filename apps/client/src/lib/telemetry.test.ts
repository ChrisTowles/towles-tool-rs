import { describe, expect, it } from "vitest";
import { DEFAULT_TELEMETRY_FILTERS, loadTelemetryFilters } from "@/lib/telemetry";

describe("loadTelemetryFilters", () => {
  it("nothing stored falls back to all defaults", () => {
    expect(loadTelemetryFilters(null)).toEqual(DEFAULT_TELEMETRY_FILTERS);
  });

  it("restores a fully valid stored value", () => {
    const raw = JSON.stringify({ level: "ERROR", kind: "span", target: "tt_exec", query: "gh" });
    expect(loadTelemetryFilters(raw)).toEqual({
      level: "ERROR",
      kind: "span",
      target: "tt_exec",
      query: "gh",
    });
  });

  it("degrades an unknown level or kind to 'all' but keeps the valid fields", () => {
    const raw = JSON.stringify({ level: "LOUD", kind: "trace", target: "x", query: "q" });
    expect(loadTelemetryFilters(raw)).toEqual({
      level: "all",
      kind: "all",
      target: "x",
      query: "q",
    });
  });

  it("keeps an arbitrary target verbatim (targets are data-dependent)", () => {
    const raw = JSON.stringify({ target: "some::module::path" });
    expect(loadTelemetryFilters(raw).target).toBe("some::module::path");
  });

  it("degrades to defaults on malformed JSON", () => {
    expect(loadTelemetryFilters("{not json")).toEqual(DEFAULT_TELEMETRY_FILTERS);
  });

  it("degrades to defaults when the stored value is not an object", () => {
    expect(loadTelemetryFilters(JSON.stringify(["a", "b"]))).toEqual(DEFAULT_TELEMETRY_FILTERS);
    expect(loadTelemetryFilters(JSON.stringify("plain"))).toEqual(DEFAULT_TELEMETRY_FILTERS);
  });

  it("fills any missing field with its default", () => {
    expect(loadTelemetryFilters(JSON.stringify({ level: "WARN" }))).toEqual({
      level: "WARN",
      kind: "all",
      target: "all",
      query: "",
    });
  });
});
