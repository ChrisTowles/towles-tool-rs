import { describe, expect, it, vi } from "vitest";
import {
  createSettingsWriter,
  nextCalendarSourceId,
  type CalendarSource,
  type UserSettings,
} from "./settings";

const source = (id: string): CalendarSource => ({ id, label: id, enabled: false, prompt: "" });

describe("nextCalendarSourceId", () => {
  it("slugs the generated label", () => {
    expect(nextCalendarSourceId([], "Calendar 1")).toBe("calendar-1");
    expect(nextCalendarSourceId([], "Work Outlook")).toBe("work-outlook");
  });

  it("suffixes until the id is free, so a new lane never collides", () => {
    const existing = [source("google"), source("google-2")];
    expect(nextCalendarSourceId(existing, "Google")).toBe("google-3");
  });

  it("collides in practice when a source is removed and re-added", () => {
    // Two sources exist, one is removed, and the next add regenerates the same
    // label — the suffix is the only thing keeping the lanes apart.
    const existing = [source("calendar-1")];
    expect(nextCalendarSourceId(existing, "Calendar 1")).toBe("calendar-1-2");
  });
});

/** Minimal settings object — only the fields these tests actually assert on. */
function base(): UserSettings {
  return {
    preferredEditor: "code",
    journalSettings: {
      baseFolder: "~/notes",
      dailyPathTemplate: "{yyyy}/daily.md",
      meetingPathTemplate: "{title}.md",
      notePathTemplate: "{title}.md",
      templateDir: "",
    },
    collectors: {
      calendar: {
        enabled: false,
        refreshMinutes: 15,
        quietHours: { enabled: false, startHour: 9, endHour: 17, weekdays: [0, 1, 2, 3, 4] },
        sources: [],
      },
      prs: { enabled: true, refreshSeconds: 120 },
      issues: { enabled: true, refreshMinutes: 10 },
      slack: {
        enabled: false,
        token: "",
        appToken: "",
        watchUserId: "",
        watchName: "",
        refreshSeconds: 60,
      },
    },
    agentboard: {},
  };
}

/**
 * Stand-in for the settings file. `load` hands back a *copy*, so a writer that
 * holds a stale snapshot is caught rather than silently reading its own cached
 * value, and `save` is deliberately slow — a write that isn't instantaneous is
 * what exposes two flushes overlapping.
 */
function fakeDisk() {
  let stored = base();
  return {
    get current() {
      return stored;
    },
    load: vi.fn<() => Promise<UserSettings | null>>(async () => structuredClone(stored)),
    save: vi.fn<(s: UserSettings) => Promise<boolean>>(async (s) => {
      await new Promise((r) => setTimeout(r, 5));
      stored = structuredClone(s);
      return true;
    }),
    /** Write directly, standing in for another part of the app touching the file. */
    async writeExternally(s: UserSettings) {
      stored = structuredClone(s);
    },
  };
}

function writerOver(disk: ReturnType<typeof fakeDisk>, deferMs = 5) {
  const states: string[] = [];
  const writer = createSettingsWriter({
    load: disk.load,
    save: disk.save,
    onState: (s) => states.push(s),
    deferMs,
  });
  return { writer, states };
}

describe("createSettingsWriter", () => {
  it("writes an immediate edit through to the file", async () => {
    const disk = fakeDisk();
    const { writer, states } = writerOver(disk);

    writer.queue((s) => ({ ...s, preferredEditor: "vim" }));
    await writer.flush();

    expect(disk.current.preferredEditor).toBe("vim");
    expect(states).toEqual(["saving", "saved"]);
  });

  it("keeps both edits when a second flush starts while the first is still writing", async () => {
    const disk = fakeDisk();
    const { writer } = writerOver(disk);

    writer.queue((s) => ({ ...s, preferredEditor: "vim" }));
    // Let the first flush claim its queue and start its (slow) write, so the
    // second edit below lands in a separate flush that overlaps it. Unserialized,
    // that second flush reads the file pre-write and reverts the editor change.
    await Promise.resolve();
    writer.queue((s) => ({
      ...s,
      collectors: { ...s.collectors, prs: { ...s.collectors.prs, enabled: false } },
    }));
    await writer.flush();

    expect(disk.current.preferredEditor).toBe("vim");
    expect(disk.current.collectors.prs.enabled).toBe(false);
  });

  it("replays edits onto a concurrent writer's changes instead of clobbering them", async () => {
    const disk = fakeDisk();
    const { writer } = writerOver(disk);

    // Someone else (terminal zoom / board group-by) writes the agentboard block
    // after this writer's screen would have loaded its copy.
    await disk.writeExternally({ ...base(), agentboard: { terminalFontSize: 22 } });

    writer.queue((s) => ({ ...s, preferredEditor: "vim" }));
    await writer.flush();

    expect(disk.current.preferredEditor).toBe("vim");
    // The concurrent write survives — this is the whole point of replaying.
    expect(disk.current.agentboard?.terminalFontSize).toBe(22);
  });

  it("coalesces deferred edits into a single write after the debounce", async () => {
    const disk = fakeDisk();
    const { writer } = writerOver(disk, 20);

    for (const name of ["a", "ab", "abc"]) {
      writer.queue((s) => ({ ...s, preferredEditor: name }), { defer: true });
    }
    expect(disk.save).not.toHaveBeenCalled();

    await new Promise((r) => setTimeout(r, 40));
    await writer.flush();

    expect(disk.save).toHaveBeenCalledTimes(1);
    expect(disk.current.preferredEditor).toBe("abc");
  });

  it("flush commits a pending deferred edit without waiting out the debounce", async () => {
    const disk = fakeDisk();
    const { writer } = writerOver(disk, 10_000);

    writer.queue((s) => ({ ...s, preferredEditor: "vim" }), { defer: true });
    await writer.flush();

    expect(disk.current.preferredEditor).toBe("vim");
  });

  it("reports an error and stays usable when a write fails", async () => {
    const disk = fakeDisk();
    const { writer, states } = writerOver(disk);
    disk.save.mockResolvedValueOnce(false);

    writer.queue((s) => ({ ...s, preferredEditor: "vim" }));
    await writer.flush();
    expect(states).toEqual(["saving", "error"]);

    // The chain survives a failure, so the next edit still writes.
    writer.queue((s) => ({ ...s, preferredEditor: "emacs" }));
    await writer.flush();
    expect(disk.current.preferredEditor).toBe("emacs");
    expect(states).toEqual(["saving", "error", "saving", "saved"]);
  });

  it("reports an error when the file can't be read", async () => {
    const disk = fakeDisk();
    const { writer, states } = writerOver(disk);
    disk.load.mockResolvedValueOnce(null);

    writer.queue((s) => ({ ...s, preferredEditor: "vim" }));
    await writer.flush();

    expect(disk.save).not.toHaveBeenCalled();
    expect(states).toEqual(["saving", "error"]);
  });

  it("does nothing when there is nothing queued", async () => {
    const disk = fakeDisk();
    const { writer, states } = writerOver(disk);

    await writer.flush();

    expect(disk.load).not.toHaveBeenCalled();
    expect(disk.save).not.toHaveBeenCalled();
    expect(states).toEqual([]);
  });
});
