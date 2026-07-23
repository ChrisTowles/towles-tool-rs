/**
 * End-to-end spec driving the real Tauri shell via @wdio/tauri-service.
 * Covers the Board screen: the store snapshot answers over real Rust IPC
 * (store_snapshot), and palette-navigating to Board renders its toolbar
 * controls. Read-only — asserts structure, never machine-specific contents,
 * and never writes state.
 */

/// <reference types="@wdio/globals/types" />
/// <reference types="@wdio/mocha-framework" />

import { expectObject } from "../ipc.js";
import { bootReady, gotoScreen } from "./nav.js";

// The camelCase snapshot the `store_snapshot` command returns (mirrors
// StoreSnapshot in apps/client/src/lib/data.ts). Only the collections the
// Board renders from are named here; each is asserted structurally.
type StoreSnapshot = {
  tasks: unknown[];
  events: unknown[];
  issues: unknown[];
  prs: unknown[];
};

describe("Board screen", () => {
  before(bootReady);

  it("answers the store snapshot over store_snapshot IPC", async () => {
    const snapshot = expectObject<StoreSnapshot>(
      await browser.tauri.execute(({ core }) => core.invoke("store_snapshot")),
      "store_snapshot",
    );
    // Structure only — the counts depend on the developer's real store.
    expect(Array.isArray(snapshot.tasks)).toBe(true);
    expect(Array.isArray(snapshot.events)).toBe(true);
    expect(Array.isArray(snapshot.issues)).toBe(true);
    expect(Array.isArray(snapshot.prs)).toBe(true);
  });

  it("navigates to Board and renders the filter control", async () => {
    await gotoScreen("Board", "Board");
    // The toolbar renders above the empty-state branch, so the filter input is
    // present regardless of how many tasks (if any) the store holds.
    const filter = await browser.$('[aria-label="Filter tasks"]');
    await filter.waitForDisplayed({ timeout: 10000 });
  });

  it("renders the group-into-swimlanes toggle", async () => {
    const swimlanes = await browser.$('[aria-label="Group tasks into repo swimlanes"]');
    await swimlanes.waitForExist({ timeout: 10000 });
  });
});
