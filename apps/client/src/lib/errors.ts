// Typed errors for the frontend's failure modes, as tagged unions rather than
// thrown `unknown`. See docs/CODING-STANDARDS.md ("Expected failures are
// values") — this is the TypeScript half of the convention the Rust crates
// already follow with `thiserror`.
import { TaggedError, isTaggedError } from "better-result";
import type { ZodError } from "zod";

/** The issue list Zod reports on a failed `safeParse`. */
type ZodIssues = ZodError["issues"];

/**
 * No Tauri host — plain-Vite browser dev. Kept distinct from a real failure so
 * a genuine backend error can't masquerade as "not wired in browser"; callers
 * that want to degrade quietly outside the shell test for it explicitly with
 * `NotInTauri.is(error)`.
 */
export class NotInTauri extends TaggedError("NotInTauri")<{
  command: string;
  message: string;
}>() {
  constructor(args: { command: string }) {
    super({ ...args, message: `not running under Tauri (${args.command})` });
  }
}

/** The Rust command rejected. `cause` is whatever Tauri's `invoke` threw. */
export class IpcFailed extends TaggedError("IpcFailed")<{
  command: string;
  cause: unknown;
  message: string;
}>() {
  constructor(args: { command: string; cause: unknown }) {
    super({ ...args, message: `${args.command}: ${describe(args.cause)}` });
  }
}

/**
 * The command resolved, but its payload didn't match the Zod schema the call
 * site declared. A backend/frontend contract drift, not a runtime failure.
 */
export class SchemaMismatch extends TaggedError("SchemaMismatch")<{
  command: string;
  issues: ZodIssues;
  message: string;
}>() {
  constructor(args: { command: string; issues: ZodIssues }) {
    const summary = args.issues
      .map((i) => `${i.path.join(".") || "(root)"}: ${i.message}`)
      .join("; ");
    super({ ...args, message: `${args.command}: response failed validation — ${summary}` });
  }
}

/**
 * The command didn't resolve within its timeout. Only abandons the *promise* —
 * Tauri commands aren't cancelable, so the backend work keeps running.
 */
export class IpcTimeout extends TaggedError("IpcTimeout")<{
  command: string;
  timeoutMs: number;
  message: string;
}>() {
  constructor(args: { command: string; timeoutMs: number }) {
    super({ ...args, message: `${args.command}: timed out after ${args.timeoutMs}ms` });
  }
}

/** Every way a Tauri command invocation can fail. */
export type IpcError = NotInTauri | IpcFailed | SchemaMismatch | IpcTimeout;

/**
 * Human-readable text for an error, for toasts and inline error UI.
 *
 * Prefer this to `String(error)`, which degrades to `"[object Object]"` on a
 * rejected Tauri command (Tauri rejects with a bare string) and on any
 * non-`Error` throw. Where the value is already typed as an {@link IpcError},
 * `error.message` reads better — tagged errors compose their own message.
 */
export function errorMessage(error: unknown): string {
  if (isTaggedError(error)) return error.message;
  return describe(error);
}

function describe(cause: unknown): string {
  if (typeof cause === "string") return cause;
  if (cause instanceof Error) return cause.message;
  if (cause === null || cause === undefined) return "unknown error";
  try {
    return JSON.stringify(cause);
  } catch {
    return String(cause);
  }
}
