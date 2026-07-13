import { z } from "zod";

/** Runtime validator for `OpenedSession` (`lib/agentboard.ts`), the
 * `ab_open_session_for_cwd` result a caller immediately navigates to (#38). */
export const OpenedSessionSchema = z.object({
  folderDir: z.string(),
  sessionId: z.string(),
});
