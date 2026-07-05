/**
 * Invoke a Tauri command from the frontend. Returns `null` in plain-Vite
 * browser dev (no Tauri host) or if the command throws, so callers can degrade
 * gracefully instead of crashing the UI.
 */
export async function invokeCmd<T>(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<T | null> {
  if (!("__TAURI_INTERNALS__" in window)) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>(cmd, args);
  } catch {
    return null;
  }
}

/**
 * Invoke a Tauri command, letting errors propagate so the caller can tell
 * success from failure (unlike {@link invokeCmd}, which flattens both to
 * `null`). Rejects if not running under Tauri.
 */
export async function invokeOrThrow<T>(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (!("__TAURI_INTERNALS__" in window)) {
    throw new Error("not running under Tauri");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}
