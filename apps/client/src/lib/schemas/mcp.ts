import { z } from "zod";

/** Runtime validator for `mcp_tool_docs` (`screens/mcp.tsx`) — the MCP
 * contract's JSON Schema tool list (`tt_mcp::tool_definitions`), worth
 * catching a shape drift on since it's rendered directly as documentation. */

const McpToolParamSchema = z.object({
  type: z.string().optional(),
  description: z.string().optional(),
  enum: z.array(z.string()).optional(),
});

export const McpToolDocSchema = z.object({
  name: z.string(),
  description: z.string(),
  inputSchema: z.object({
    type: z.string(),
    properties: z.record(z.string(), McpToolParamSchema).default({}),
    required: z.array(z.string()).default([]),
  }),
  /** MCP's own tool annotations. The server emits `readOnlyHint: false` on the
   * tools that write and omits the whole block otherwise, so absence means
   * "no claim made", not "read-only". */
  annotations: z.object({ readOnlyHint: z.boolean().optional() }).optional(),
});

export const McpToolDocsSchema = z.array(McpToolDocSchema);

export type McpToolDoc = z.infer<typeof McpToolDocSchema>;

/** Runtime validator for `mcp_status` — whether *this* instance won the bind
 * race for the MCP port, and which port that is. */
export const McpStatusSchema = z.object({ serving: z.boolean(), port: z.number() });

export type McpStatus = z.infer<typeof McpStatusSchema>;

/** Runtime validator for `mcp_test_call` — what one real round-trip against the
 * MCP endpoint came back with. A refusal is a result to display, not an error:
 * the point is seeing what a client would see. `sentOrigin` records whether the
 * request deliberately carried an `Origin` header. */
export const McpTestResultSchema = z.object({
  status: z.number(),
  body: z.string(),
  durationMs: z.number(),
  sentOrigin: z.boolean(),
});

export type McpTestResult = z.infer<typeof McpTestResultSchema>;
