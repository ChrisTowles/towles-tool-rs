import type { ZodType } from "zod";
import { toast } from "sonner";

/** True when running inside the Tauri shell (vs. plain-Vite browser dev). */
export const isTauri = () => "__TAURI_INTERNALS__" in window;

/**
 * Invoke a Tauri command from the frontend. Returns `null` in plain-Vite
 * browser dev (no Tauri host) or if the command throws, so callers can degrade
 * gracefully instead of crashing the UI.
 *
 * An optional Zod `schema` validates the response at this boundary (#38) — a
 * shape mismatch is logged and treated the same as any other failure (`null`),
 * matching this function's existing "never throws" contract. Most call sites
 * still omit it; adoption is intentionally staged to the highest-risk
 * boundaries (shared on-disk settings, external API payloads) rather than a
 * full sweep in one pass.
 */
export async function invokeCmd<T>(
  cmd: string,
  args: Record<string, unknown> = {},
  schema?: ZodType<T>,
): Promise<T | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    const result = await invoke<T>(cmd, args);
    if (!schema) return result;
    const parsed = schema.safeParse(result);
    if (!parsed.success) {
      console.error(`invokeCmd(${cmd}): response failed schema validation`, parsed.error.issues);
      return null;
    }
    return parsed.data;
  } catch {
    return null;
  }
}

/**
 * Invoke a Tauri command, letting errors propagate so the caller can tell
 * success from failure (unlike {@link invokeCmd}, which flattens both to
 * `null`). Rejects if not running under Tauri.
 *
 * An optional Zod `schema` validates the response (#38); a mismatch throws,
 * matching this function's existing "let errors propagate" contract.
 */
export async function invokeOrThrow<T>(
  cmd: string,
  args: Record<string, unknown> = {},
  schema?: ZodType<T>,
): Promise<T> {
  if (!isTauri()) {
    throw new Error("not running under Tauri");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await invoke<T>(cmd, args);
  if (!schema) return result;
  const parsed = schema.safeParse(result);
  if (!parsed.success) {
    throw new Error(`invokeOrThrow(${cmd}): response failed schema validation: ${parsed.error.message}`);
  }
  return parsed.data;
}

/**
 * Read-style command wrapper for optional data: `null` in browser dev (silently)
 * or on failure (after surfacing the error as a toast). Shared by the journal
 * and claude-sessions bridges, which return real payloads or nothing.
 */
export async function invokeToast<T>(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<T | null> {
  if (!isTauri()) return null;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<T>(cmd, args);
  } catch (e) {
    toast.error(String(e));
    return null;
  }
}

/**
 * Write-style command wrapper: `true` on success, `false` in browser dev (with
 * an info toast) or on failure (after an error toast). Distinct from
 * {@link invokeToast} because a void Tauri command resolves to `null`, so a
 * `T | null` result can't tell success from failure — callers that revert an
 * optimistic update need the boolean.
 */
export async function invokeOk(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<boolean> {
  if (!isTauri()) {
    toast.info("not wired in browser");
    return false;
  }
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke(cmd, args);
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  }
}
