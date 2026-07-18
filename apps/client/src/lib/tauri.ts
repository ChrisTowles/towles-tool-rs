import type { ZodType } from "zod";
import { Result } from "better-result";
import { IpcFailed, IpcTimeout, NotInTauri, SchemaMismatch } from "@/lib/errors";
import type { IpcError } from "@/lib/errors";

/** True when running inside the Tauri shell (vs. plain-Vite browser dev). */
export const isTauri = () => "__TAURI_INTERNALS__" in window;

/** Per-call knobs. Both are off by default. */
export type InvokeOptions<T> = {
  /**
   * Validates the response at this boundary. A mismatch is a
   * {@link SchemaMismatch} error, not a thrown exception — backend/frontend
   * contract drift is an expected failure, not a defect.
   */
  schema?: ZodType<T>;
  /**
   * Fails the call with {@link IpcTimeout} once elapsed, so a command whose
   * backend work never resolves can't leave an "in progress" UI state stuck
   * forever. This only abandons *this* promise — the backend command keeps
   * running to completion regardless, since Tauri commands aren't cancelable.
   */
  timeoutMs?: number;
};

/**
 * Invoke a Tauri command. Never throws and never rejects: every failure —
 * no Tauri host, a rejected command, a schema mismatch, a timeout — comes back
 * as a typed `Err` in the {@link IpcError} union.
 *
 * Because failure is a value, each call site picks its own failure UX rather
 * than inheriting one from the function it happened to call. The three shapes
 * in use here:
 *
 * ```ts
 * // Degrade quietly to a fallback.
 * const repos = (await invoke<Repo[]>("list_repos")).unwrapOr([]);
 *
 * // Surface real failures, but stay silent in plain-Vite browser dev.
 * (await invoke<View>("load_view")).match({
 *   ok: setView,
 *   err: (e) => { if (!NotInTauri.is(e)) toast.error(e.message); },
 * });
 *
 * // Branch on the outcome.
 * if ((await invoke("store_delete_task", { id })).isErr()) revertOptimisticDelete();
 * ```
 *
 * Fire-and-forget is safe by construction: an ignored `Result` can't produce an
 * unhandled rejection, so the hot PTY-write path needs no `.catch`.
 */
export async function invoke<T>(
  cmd: string,
  args: Record<string, unknown> = {},
  options: InvokeOptions<T> = {},
): Promise<Result<T, IpcError>> {
  if (!isTauri()) return Result.err(new NotInTauri({ command: cmd }));

  const { schema, timeoutMs } = options;
  const core = await import("@tauri-apps/api/core");

  // The command is invoked inside the thunk, not before it, so each attempt is
  // a fresh call — re-awaiting one already-settled promise would make any
  // future `retry` config a silent no-op.
  const settled = await Result.tryPromise({
    try: () => {
      const call = core.invoke<T>(cmd, args);
      return timeoutMs === undefined ? call : withTimeout(call, timeoutMs, cmd);
    },
    catch: (cause): IpcError =>
      IpcTimeout.is(cause) ? cause : new IpcFailed({ command: cmd, cause }),
  });

  return settled.andThen((value) => {
    if (!schema) return Result.ok<T, IpcError>(value);
    const parsed = schema.safeParse(value);
    return parsed.success
      ? Result.ok<T, IpcError>(parsed.data)
      : Result.err<T, IpcError>(new SchemaMismatch({ command: cmd, issues: parsed.error.issues }));
  });
}

/**
 * Rejects with {@link IpcTimeout} once `ms` elapses. Rejecting (rather than
 * resolving an `Err`) keeps this composable with `Result.tryPromise` above,
 * which classifies it back into the error union.
 */
function withTimeout<T>(promise: Promise<T>, ms: number, command: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new IpcTimeout({ command, timeoutMs: ms })), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}
