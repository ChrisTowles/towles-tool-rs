/**
 * End-to-end spec driving the real Tauri shell via @wdio/tauri-service.
 * Covers the Settings screen UI: palette-navigate to it, then exercise the
 * sub-tab panel (switch tabs, assert the selected pane renders).
 *
 * READ-ONLY by construction — it only navigates and switches tabs. It never
 * touches an input, never clicks Save, and so never writes the real settings
 * file (a hard rule from CLAUDE.md; the settings file is shared with the
 * TypeScript CLI). settings.e2e.ts is the IPC-level settings smoke test; this
 * is its UI counterpart.
 */

/// <reference types="@wdio/globals/types" />
/// <reference types="@wdio/mocha-framework" />

import { bootReady, clickTab, expectTabPanelShown, gotoScreen } from "./nav.js";

describe("Settings screen UI", () => {
  before(bootReady);

  it("navigates to Settings and renders its tab list", async () => {
    await gotoScreen("Settings", "Settings");
    await browser.waitUntil(
      async () => (await browser.$$('[data-slot="tabs-trigger"]').length) > 0,
      { timeout: 10000, timeoutMsg: "settings tab list never rendered" },
    );
  });

  it("switches to the Appearance tab and shows its pane", async () => {
    await clickTab("Appearance");
    await expectTabPanelShown("Appearance");
  });

  it("switches to the About tab and shows its pane", async () => {
    await clickTab("About");
    await expectTabPanelShown("About");
  });
});
