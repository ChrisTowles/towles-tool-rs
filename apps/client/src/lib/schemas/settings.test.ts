import { describe, expect, it } from "vitest";
import { UserSettingsSchema } from "./settings";

const valid = {
  preferredEditor: "code",
  journalSettings: {
    baseFolder: "~/journal",
    dailyPathTemplate: "{{date}}.md",
    meetingPathTemplate: "{{date}}-meeting.md",
    notePathTemplate: "{{date}}-note.md",
    templateDir: "~/journal/templates",
  },
  promptImprovers: [
    { id: "direct", label: "Direct", enabled: true, preferred: true, prompt: "Restate it." },
  ],
  collectors: {
    calendar: {
      enabled: false,
      refreshMinutes: 15,
      quietHours: { enabled: false, startHour: 9, endHour: 17, weekdays: [0, 1, 2, 3, 4] },
      sources: [{ id: "google", label: "Google (personal)", enabled: true, prompt: "list today" }],
    },
    prs: { enabled: true, refreshSeconds: 60, mergedRefreshMinutes: 15 },
    issues: { enabled: true, refreshMinutes: 5 },
    slack: {
      enabled: false,
      token: "",
      appToken: "",
      watchUserId: "",
      watchName: "",
      refreshSeconds: 60,
    },
  },
};

describe("UserSettingsSchema", () => {
  it("parses a well-formed settings object", () => {
    expect(UserSettingsSchema.parse(valid)).toEqual(valid);
  });

  it("keeps unknown top-level fields (TS-CLI compat)", () => {
    const withExtra = { ...valid, someFutureField: "kept" };
    expect(UserSettingsSchema.parse(withExtra)).toMatchObject({ someFutureField: "kept" });
  });

  it("keeps unknown agentboard keys", () => {
    const withAgentboard = { ...valid, agentboard: { compactRecommendPercent: 40, futureKey: 1 } };
    const parsed = UserSettingsSchema.parse(withAgentboard);
    expect(parsed.agentboard).toEqual({ compactRecommendPercent: 40, futureKey: 1 });
  });

  it("rejects a payload missing a required field", () => {
    const { preferredEditor: _preferredEditor, ...missing } = valid;
    expect(() => UserSettingsSchema.parse(missing)).toThrow("preferredEditor");
  });
});
