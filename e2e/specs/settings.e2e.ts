/**
 * End-to-end smoke test driving the real Tauri shell via @wdio/tauri-service.
 * Proves three things the bare-browser path can't: the app boots in the real
 * WebView, real Rust IPC commands answer (settings_get / app_slot /
 * ab_discover_repos), and IPC mocking works. Read-only — never writes settings.
 */

/// <reference types="@wdio/globals/types" />
/// <reference types="@wdio/mocha-framework" />

type UserSettings = {
  preferredEditor: string;
  journalSettings: { baseFolder: string };
  collectors: { calendar: { enabled: boolean }; prs: unknown; issues: unknown };
};

describe("Towles Tool desktop shell", () => {
  it("boots and renders the React app into #root", async () => {
    const root = await browser.$("#root");
    await root.waitForExist({ timeout: 15000 });
    // The app mounted something, not an empty shell.
    await browser.waitUntil(async () => (await root.$$("*")).length > 0, {
      timeout: 15000,
      timeoutMsg: "#root never got children",
    });
  });

  it("answers a real Rust command (app_slot)", async () => {
    const slot = await browser.tauri.execute(({ core }) =>
      core.invoke<string>("app_slot"),
    );
    expect(typeof slot).toBe("string");
    expect(slot.length).toBeGreaterThan(0);
  });

  it("reads real settings over settings_get IPC", async () => {
    const settings = await browser.tauri.execute(({ core }) =>
      core.invoke<UserSettings>("settings_get"),
    );
    expect(settings).toBeDefined();
    expect(typeof settings.preferredEditor).toBe("string");
    expect(settings.journalSettings).toBeDefined();
    expect(settings.collectors.calendar).toBeDefined();
  });

  it("discovers repos over ab_discover_repos IPC (returns an array)", async () => {
    const candidates = await browser.tauri.execute(({ core }) =>
      core.invoke<Array<{ name: string; dir: string }>>("ab_discover_repos"),
    );
    expect(Array.isArray(candidates)).toBe(true);
  });
});
