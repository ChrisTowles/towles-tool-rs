/**
 * End-to-end spec driving the real Tauri shell via @wdio/tauri-service.
 * Covers the Telemetry screen: palette-navigate to it, assert the day-picker
 * and tab UI render, switch to the Log tab to reveal its search filter, and
 * prove the read commands (telemetry_days / telemetry_events) answer without
 * error. Read-only — asserts structure, never log contents, never writes state.
 */

/// <reference types="@wdio/globals/types" />
/// <reference types="@wdio/mocha-framework" />

import { expectArray } from "../ipc.js";
import { bootReady, clickTab, gotoScreen } from "./nav.js";

describe("Telemetry screen", () => {
  before(bootReady);

  it("navigates to Telemetry and renders the day picker", async () => {
    await gotoScreen("Telemetry");
    // The day picker lives in the always-visible header (a shadcn Select).
    const dayPicker = await browser.$('[data-slot="select-trigger"]');
    await dayPicker.waitForDisplayed({ timeout: 10000 });
  });

  it("renders the Overview/Log/Insights tabs", async () => {
    await browser.waitUntil(
      async () => (await browser.$$('[data-slot="tabs-trigger"]').length) >= 3,
      { timeout: 10000, timeoutMsg: "telemetry tab list never rendered" },
    );
  });

  it("switches to the Log tab and shows the search filter", async () => {
    await clickTab("Log");
    const search = await browser.$('input[placeholder="Search target, name, fields…"]');
    await search.waitForDisplayed({ timeout: 10000 });
  });

  it("lists available days over telemetry_days IPC", async () => {
    const days = expectArray(
      await browser.tauri.execute(({ core }) => core.invoke("telemetry_days")),
      "telemetry_days",
    );
    // Structure only — the set of days depends on the developer's real log.
    expect(Array.isArray(days)).toBe(true);
  });

  it("queries a day's events over telemetry_events without error", async () => {
    // Resolve the day and fetch its events entirely inside the WebView so no
    // argument has to cross the execute boundary. If the log is empty, a
    // well-formed date the backend simply answers with `[]` keeps this a pure
    // structural check.
    const events = expectArray(
      await browser.tauri.execute(async ({ core }) => {
        const days = await core.invoke("telemetry_days");
        const date =
          Array.isArray(days) && typeof days[0] === "string" ? days[0] : "2020-01-01";
        return core.invoke("telemetry_events", { date });
      }),
      "telemetry_events",
    );
    expect(Array.isArray(events)).toBe(true);
  });
});
