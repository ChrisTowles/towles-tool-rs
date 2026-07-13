import { z } from "zod";

/** Runtime validator for `DoctorReport` (`screens/doctor.tsx`), the
 * `doctor_run` command's result — a few subprocess-probed checks, worth
 * catching a shape drift on rather than rendering a blank screen (#38). */

const CheckResultSchema = z.object({
  name: z.string(),
  version: z.string().nullable(),
  ok: z.boolean(),
  warning: z.string().optional(),
});

const NameOkSchema = z.object({
  name: z.string(),
  ok: z.boolean(),
});

const PluginCheckSchema = z.object({
  name: z.string(),
  ok: z.boolean(),
  installHint: z.string().optional(),
});

const AgentBoardCheckSchema = z.object({
  name: z.string(),
  value: z.string(),
  ok: z.boolean(),
  warning: z.string().optional(),
  hint: z.string().optional(),
});

export const DoctorReportSchema = z.object({
  result: z.object({
    timestamp: z.string(),
    tools: z.array(CheckResultSchema),
    ghAuth: z.boolean(),
    plugins: z.array(NameOkSchema),
    agentboard: z.array(NameOkSchema),
  }),
  plugins: z.array(PluginCheckSchema),
  agentboard: z.array(AgentBoardCheckSchema),
});
