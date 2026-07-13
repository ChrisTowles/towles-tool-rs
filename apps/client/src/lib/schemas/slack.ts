import { z } from "zod";

/** Runtime validators for the Slack DM panel (`lib/slack.ts`) — payloads that
 * originate from the external Slack Web API, one layer further from our own
 * control than an internal command result (#38). */

const DmFileSchema = z.object({
  id: z.string(),
  name: z.string(),
  mimetype: z.string(),
  urlPrivate: z.string(),
  thumbUrl: z.string(),
  permalink: z.string(),
  isImage: z.boolean(),
});

const DmMessageSchema = z.object({
  text: z.string(),
  ts: z.number(),
  fromMe: z.boolean(),
  files: z.array(DmFileSchema),
});

export const SlackDmViewSchema = z.object({
  configured: z.boolean(),
  watchName: z.string(),
  watchUserId: z.string(),
  messages: z.array(DmMessageSchema),
});

export const SlackFileDataSchema = z.object({
  mimetype: z.string(),
  dataBase64: z.string(),
});
