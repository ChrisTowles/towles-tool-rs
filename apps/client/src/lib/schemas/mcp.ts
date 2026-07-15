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
});

export const McpToolDocsSchema = z.array(McpToolDocSchema);

export type McpToolDoc = z.infer<typeof McpToolDocSchema>;
