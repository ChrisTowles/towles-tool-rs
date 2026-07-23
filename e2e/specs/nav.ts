/**
 * Shared helpers for the WebdriverIO specs that drive the real Tauri shell.
 * Not a spec itself (no `.e2e.ts` suffix, so wdio.conf's `specs` glob skips it)
 * — just the boot/navigation/tab primitives the individual screen specs reuse,
 * factored out of the patterns first written inline in palette.e2e.ts.
 *
 * Everything here is read-only: it navigates and inspects, never writing
 * settings or other persisted state.
 */

/// <reference types="@wdio/globals/types" />
/// <reference types="@wdio/mocha-framework" />

/** Wait until the React app has mounted real content into #root. */
export async function bootReady(): Promise<void> {
  const root = await browser.$("#root");
  await root.waitForExist({ timeout: 15000 });
  await browser.waitUntil(async () => (await root.$$("*").length) > 0, {
    timeout: 15000,
    timeoutMsg: "#root never got children",
  });
}

/**
 * Wait until an active (aria-current) sidebar control is labelled `title`.
 * Expanded it's a visible-text button; icon-collapsed (the e2e default) it's
 * icon-only with the title as `aria-label` — check both so the assertion holds
 * regardless of collapse state.
 */
export async function expectActiveScreen(title: string): Promise<void> {
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
    { timeout: 10000, timeoutMsg: `no active screen titled "${title}"` },
  );
}

/**
 * Navigate to a screen by clicking its sidebar nav button, then assert it
 * becomes active.
 *
 * Deliberately not the ⌘K command palette: opening the palette needs a
 * synthetic Ctrl/⌘+K chord to land on the right focus target, which is
 * unreliable under WebKitGTK automation (the chord silently no-ops when focus
 * sits in an input or the just-restored screen — e.g. booting onto Settings —
 * so the palette never opens). Clicking the sidebar button drives `openTab`
 * directly: a plain W3C element click, deterministic regardless of focus or
 * which screen the session restored. The icon-collapsed sidebar (the e2e
 * default) labels the button via `aria-label`; expanded mode via visible text.
 * palette.e2e.ts still covers the palette itself.
 */
export async function gotoScreen(title: string): Promise<void> {
  await clickNavButton(title);
  await expectActiveScreen(title);
}

/** Click a sidebar nav button by title (aria-label collapsed, text expanded). */
async function clickNavButton(title: string): Promise<void> {
  const byLabel = await browser.$(`button[aria-label="${title}"]`);
  if (await byLabel.isExisting()) {
    await byLabel.click();
    return;
  }
  const buttons = await browser.$$("button");
  for (const button of buttons) {
    if ((await button.getText()).trim() === title) {
      await button.click();
      return;
    }
  }
  throw new Error(`no sidebar nav button titled "${title}"`);
}

/**
 * Click a shadcn/Radix tab by its visible label and wait until it reports
 * `aria-selected`. Matches on trimmed visible text, so hidden triggers from
 * other still-mounted screens (App.tsx keeps screens mounted, only hidden)
 * don't collide — their `getText()` is empty.
 */
export async function clickTab(label: string): Promise<void> {
  const triggers = await browser.$$('[data-slot="tabs-trigger"]');
  for (const trigger of triggers) {
    if ((await trigger.getText()).trim() === label) {
      await trigger.click();
      break;
    }
  }
  await browser.waitUntil(
    async () => {
      const selected = await browser.$$('[data-slot="tabs-trigger"][aria-selected="true"]');
      for (const trigger of selected) {
        if ((await trigger.getText()).trim() === label) return true;
      }
      return false;
    },
    { timeout: 10000, timeoutMsg: `tab "${label}" never became selected` },
  );
}

/**
 * Assert the panel bound to the selected tab `label` is displayed. Radix links
 * a trigger to its content via `aria-controls` → the panel's `id`, so this
 * resolves the exact panel that tab controls rather than guessing at DOM order
 * (other mounted screens also render `tabs-content` nodes).
 */
export async function expectTabPanelShown(label: string): Promise<void> {
  const selected = await browser.$$('[data-slot="tabs-trigger"][aria-selected="true"]');
  for (const trigger of selected) {
    if ((await trigger.getText()).trim() === label) {
      const panelId = await trigger.getAttribute("aria-controls");
      if (panelId) {
        // Attribute selector, not `#id`: Radix panel ids contain colons.
        const panel = await browser.$(`[id="${panelId}"]`);
        await panel.waitForDisplayed({ timeout: 10000 });
        return;
      }
    }
  }
  throw new Error(`no displayed panel for selected tab "${label}"`);
}
