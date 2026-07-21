import { z } from "zod";

/**
 * Runtime validators for `UserSettings` (`lib/settings.ts`) — the shared
 * settings file every worktree reads/writes, hand-edited on disk and
 * carried across a TS-CLI compatibility boundary (#38). Mirrors the TS type
 * field-for-field; `.passthrough()` on every object matches the Rust side's
 * `#[serde(default)]`/no-`deny_unknown_fields` tolerance so a field this
 * schema doesn't know about yet survives a validated read instead of being
 * silently dropped from what gets saved back.
 */

const JournalSettingsSchema = z
  .object({
    baseFolder: z.string(),
    dailyPathTemplate: z.string(),
    meetingPathTemplate: z.string(),
    notePathTemplate: z.string(),
    templateDir: z.string(),
  })
  .passthrough();

const CalendarQuietHoursSchema = z
  .object({
    enabled: z.boolean(),
    startHour: z.number(),
    endHour: z.number(),
    weekdays: z.array(z.number()),
  })
  .passthrough();

const CalendarSourceSchema = z
  .object({
    id: z.string(),
    label: z.string(),
    enabled: z.boolean(),
    prompt: z.string(),
  })
  .passthrough();

const PromptImproverSchema = z
  .object({
    id: z.string(),
    label: z.string(),
    enabled: z.boolean(),
    preferred: z.boolean(),
    prompt: z.string(),
  })
  .passthrough();

const CalendarCollectorSchema = z
  .object({
    enabled: z.boolean(),
    refreshMinutes: z.number(),
    quietHours: CalendarQuietHoursSchema,
    sources: z.array(CalendarSourceSchema),
  })
  .passthrough();

const PrCollectorSchema = z
  .object({
    enabled: z.boolean(),
    refreshSeconds: z.number(),
  })
  .passthrough();

const IssueCollectorSchema = z
  .object({
    enabled: z.boolean(),
    refreshMinutes: z.number(),
  })
  .passthrough();

const SlackDmCollectorSchema = z
  .object({
    enabled: z.boolean(),
    token: z.string(),
    appToken: z.string(),
    watchUserId: z.string(),
    watchName: z.string(),
    refreshSeconds: z.number(),
  })
  .passthrough();

const CollectorsSettingsSchema = z
  .object({
    calendar: CalendarCollectorSchema,
    prs: PrCollectorSchema,
    issues: IssueCollectorSchema,
    slack: SlackDmCollectorSchema,
  })
  .passthrough();

/** `UserSettings["agentboard"]` is a TS-owned, partly-typed block that already
 * carries a `Record<string, unknown>` passthrough in its TS type — `.catchall`
 * keeps every key not listed here rather than requiring an exhaustive list. */
const AgentboardBlockSchema = z
  .object({
    notifyNeedsYou: z.boolean().optional(),
    notifyMeetingStart: z.boolean().optional(),
    notifyReviewRequested: z.boolean().optional(),
    notifyChecksFailed: z.boolean().optional(),
    notifyStaleCollector: z.boolean().optional(),
    compactRecommendPercent: z.number().optional(),
    copyOnSelect: z.boolean().optional(),
    terminalFontSize: z.number().optional(),
    shortcutsWorkInTerminal: z.boolean().optional(),
    boardGroupByRepo: z.boolean().optional(),
  })
  .catchall(z.unknown());

export const UserSettingsSchema = z
  .object({
    preferredEditor: z.string(),
    journalSettings: JournalSettingsSchema,
    promptImprovers: z.array(PromptImproverSchema),
    collectors: CollectorsSettingsSchema,
    agentboard: AgentboardBlockSchema.optional(),
  })
  .passthrough();
