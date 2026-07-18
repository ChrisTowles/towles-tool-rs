/**
 * End-to-end smoke test driving the real Tauri shell via @wdio/tauri-service.
 * Exercises the command palette the way a user does — open with Ctrl/Cmd+K,
 * type, Enter — and proves navigation lands on the target screen. Also checks
 * the slot badge, whose value comes from the real `app_slot` Rust command.
 * Read-only — never writes settings or other state.
 */

/// <reference types="@wdio/globals/types" />
/// <reference types="@wdio/mocha-framework" />

import { Key } from "webdriverio";
import { expectString } from "../ipc.js";

// The app binds the palette to ⌘K on macOS, Ctrl+K everywhere else (mirrors the
// frontend's IS_MAC). The suite runs on Linux/WebKitGTK, but keep it portable.
const MOD = process.platform === "darwin" ? Key.Command : Key.Ctrl;

/** Open the palette via the real keyboard shortcut and wait for its input. */
async function openPalette(): Promise<void> {
  // Focus the window chrome so the chord reaches the global keydown listener.
  await browser.$("#root").click();
  await browser.keys([MOD, "k"]);
  const input = await browser.$('[data-slot="command-input"]');
  await input.waitForDisplayed({ timeout: 10000 });
}

/** Type a query, select the top-ranked item, and wait for the palette to close. */
async function navigateTo(query: string): Promise<void> {
  const input = await browser.$('[data-slot="command-input"]');
  await input.setValue(query);
  await browser.keys(Key.Enter);
  await browser
    .$('[data-slot="command-input"]')
    .waitForExist({ reverse: true, timeout: 10000 });
}

/**
 * Wait until an active (aria-current) sidebar control is labelled `title`.
 * Expanded, it's a visible-text button (`AppSidebar`); icon-collapsed (the
 * e2e default), it's icon-only with the title as `aria-label`
 * (`AppSidebarIcons`) — check both so the assertion holds regardless of
 * collapse state.
 */
async function expectActiveTab(title: string): Promise<void> {
  await browser.waitUntil(
    async () => {
      const tabs = await browser.$$('button[aria-current="true"]');
      for (const tab of tabs) {
        const text = (await tab.getText()).trim();
        const label = await tab.getAttribute("aria-label");
        if (text === title || label === title) return true;
      }
      return false;
    },
    { timeout: 10000, timeoutMsg: `no active tab titled "${title}"` },
  );
}

describe("Command palette navigation", () => {
  before(async () => {
    const root = await browser.$("#root");
    await root.waitForExist({ timeout: 15000 });
    await browser.waitUntil(async () => (await root.$$("*").length) > 0, {
      timeout: 15000,
      timeoutMsg: "#root never got children",
    });
  });

  it("shows the slot badge from the real app_slot command", async () => {
    const slot = expectString(
      await browser.tauri.execute(({ core }) => core.invoke("app_slot")),
      "app_slot",
    );
    expect(slot.length).toBeGreaterThan(0);
    // The header badge carries the full slot as its title attribute.
    const badge = await browser.$(`[title="${slot}"]`);
    await badge.waitForDisplayed({ timeout: 10000 });
    expect((await badge.getText()).length).toBeGreaterThan(0);
  });

  it("navigates to Board via the palette", async () => {
    await openPalette();
    await navigateTo("Board");
    await expectActiveTab("Board");
  });

  it("navigates to Agentboard via the palette", async () => {
    await openPalette();
    await navigateTo("Agentboard");
    await expectActiveTab("Agentboard");
  });
});
