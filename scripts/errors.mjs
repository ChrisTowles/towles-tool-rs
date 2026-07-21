// Typed failures for the dev-tooling scripts, as tagged errors rather than
// thrown `unknown` or a `null` that conflates several causes. See
// docs/CODING-STANDARDS.md ("Expected failures are values"); this is the
// scripts-side twin of apps/client/src/lib/errors.ts.
//
// `TaggedError("Tag")<Props>()` is TypeScript syntax and these are plain
// `.mjs` files, so each class casts the factory's return to
// `TaggedErrorClass<Tag, Props>` to declare its props. The cast is the only
// way to name Props without a `.d.mts` sidecar, and it keeps `Tag.is(value)`
// and the props typed at every call site.
import { TaggedError } from "better-result";

/**
 * @typedef {object} EnvFileUnreadableProps
 * @property {string} path
 * @property {unknown} cause
 * @property {string} message
 */

/** @type {import("better-result").TaggedErrorClass<"EnvFileUnreadable", EnvFileUnreadableProps>} */
const EnvFileUnreadableBase = TaggedError("EnvFileUnreadable")();

/**
 * An env file exists but could not be read (permissions, a directory in its
 * place, I/O). Deliberately distinct from "absent", which is the normal case
 * for a checkout with no `.env.local` and is not an error at all.
 */
export class EnvFileUnreadable extends EnvFileUnreadableBase {
  /** @param {{ path: string; cause: unknown }} args */
  constructor(args) {
    super({ ...args, message: `could not read ${args.path}: ${describe(args.cause)}` });
  }
}

/**
 * @typedef {object} DevPortUnsetProps
 * @property {string} message
 */

/** @type {import("better-result").TaggedErrorClass<"DevPortUnset", DevPortUnsetProps>} */
const DevPortUnsetBase = TaggedError("DevPortUnset")();

/**
 * No `TT_DEV_PORT` anywhere — shell env, `.env.local`, or the rendered `.env`.
 * Recoverable: the launchers render the task's `.env` and retry. Kept separate
 * from {@link DevPortInvalid}, which is a typo the user has to fix.
 */
export class DevPortUnset extends DevPortUnsetBase {
  constructor() {
    super({ message: "no TT_DEV_PORT for this checkout" });
  }
}

/**
 * @typedef {object} DevPortInvalidProps
 * @property {string} value
 * @property {string} message
 */

/** @type {import("better-result").TaggedErrorClass<"DevPortInvalid", DevPortInvalidProps>} */
const DevPortInvalidBase = TaggedError("DevPortInvalid")();

/** `TT_DEV_PORT` is set to something that isn't a port number in 1-65535. */
export class DevPortInvalid extends DevPortInvalidBase {
  /** @param {{ value: string }} args */
  constructor(args) {
    super({ ...args, message: `TT_DEV_PORT=${args.value} is not a valid port (1-65535)` });
  }
}

/**
 * @typedef {object} TaskEnvRenderFailedProps
 * @property {string} name
 * @property {unknown} cause
 * @property {string} message
 */

/**
 * @type {import("better-result").TaggedErrorClass<
 *   "TaskEnvRenderFailed", TaskEnvRenderFailedProps>}
 */
const TaskEnvRenderFailedBase = TaggedError("TaskEnvRenderFailed")();

/** `tt task env <name>` could not run or exited non-zero — `tt` missing, or the render failed. */
export class TaskEnvRenderFailed extends TaskEnvRenderFailedBase {
  /** @param {{ name: string; cause: unknown }} args */
  constructor(args) {
    super({
      ...args,
      message: `\`tt task env ${args.name}\` failed: ${describe(args.cause)}`,
    });
  }
}

/**
 * @typedef {object} SpawnFailedProps
 * @property {string} command
 * @property {unknown} cause
 * @property {string} message
 */

/** @type {import("better-result").TaggedErrorClass<"SpawnFailed", SpawnFailedProps>} */
const SpawnFailedBase = TaggedError("SpawnFailed")();

/**
 * A child process never started — the binary is missing or not executable.
 * Distinct from a process that ran and exited non-zero, which is an exit code,
 * not a failure to launch.
 */
export class SpawnFailed extends SpawnFailedBase {
  /** @param {{ command: string; cause: unknown }} args */
  constructor(args) {
    super({ ...args, message: `could not run \`${args.command}\`: ${describe(args.cause)}` });
  }
}

/**
 * @typedef {object} BadVersionProps
 * @property {string} version
 * @property {string} message
 */

/** @type {import("better-result").TaggedErrorClass<"BadVersion", BadVersionProps>} */
const BadVersionBase = TaggedError("BadVersion")();

/** A `plugin.json` version that isn't `major.minor.patch`. */
export class BadVersion extends BadVersionBase {
  /** @param {{ version: string }} args */
  constructor(args) {
    super({
      ...args,
      message: `plugin.json version "${args.version}" is not major.minor.patch`,
    });
  }
}

/**
 * @typedef {object} VersionLineMissingProps
 * @property {string} needle
 * @property {string} message
 */

/**
 * @type {import("better-result").TaggedErrorClass<
 *   "VersionLineMissing", VersionLineMissingProps>}
 */
const VersionLineMissingBase = TaggedError("VersionLineMissing")();

/** The manifest has no `"version": "<from>"` line to rewrite. */
export class VersionLineMissing extends VersionLineMissingBase {
  /** @param {{ needle: string }} args */
  constructor(args) {
    super({ ...args, message: `could not find ${args.needle} to replace` });
  }
}

/**
 * @typedef {object} PortNeverListenedProps
 * @property {number} port
 * @property {number} timeoutMs
 * @property {string} message
 */

/** @type {import("better-result").TaggedErrorClass<"PortNeverListened", PortNeverListenedProps>} */
const PortNeverListenedBase = TaggedError("PortNeverListened")();

/** Nothing accepted a connection on the port within the timeout. */
export class PortNeverListened extends PortNeverListenedBase {
  /** @param {{ port: number; timeoutMs: number }} args */
  constructor(args) {
    super({ ...args, message: `port ${args.port} not up in ${args.timeoutMs}ms` });
  }
}

/**
 * @typedef {object} RequestFailedProps
 * @property {string} url
 * @property {unknown} cause
 * @property {string} message
 */

/** @type {import("better-result").TaggedErrorClass<"RequestFailed", RequestFailedProps>} */
const RequestFailedBase = TaggedError("RequestFailed")();

/** The automation server was unreachable — nothing answered on the socket. */
export class RequestFailed extends RequestFailedBase {
  /** @param {{ url: string; base: string; cause: unknown }} args */
  constructor(args) {
    const code = errnoCode(args.cause) ?? describe(args.cause);
    super({
      url: args.url,
      cause: args.cause,
      message:
        `can't reach the automation server at ${args.base} (${code}).\n` +
        "Is `npm run dev:drive` running in this task?",
    });
  }
}

/**
 * @typedef {object} RemoteRejectedProps
 * @property {string} detail
 * @property {string} message
 */

/** @type {import("better-result").TaggedErrorClass<"RemoteRejected", RemoteRejectedProps>} */
const RemoteRejectedBase = TaggedError("RemoteRejected")();

/**
 * The automation server answered, but with a failure — a non-2xx status, a
 * WebDriver error body, or a payload missing the field the caller needed.
 */
export class RemoteRejected extends RemoteRejectedBase {
  /** @param {{ action: string; detail: string }} args */
  constructor(args) {
    super({ detail: args.detail, message: `${args.action}: ${args.detail}` });
  }
}

/**
 * The `code` of a Node system error (`ECONNREFUSED`, `ENOENT`, …), when there
 * is one. `fetch` buries it one level down, in the `TypeError`'s `cause`.
 *
 * @param {unknown} cause
 * @returns {string | undefined}
 */
function errnoCode(cause) {
  const nested = cause instanceof Error && cause.cause !== undefined ? cause.cause : cause;
  if (!(nested instanceof Error)) return undefined;
  const code = /** @type {NodeJS.ErrnoException} */ (nested).code;
  return typeof code === "string" ? code : undefined;
}

/**
 * Human-readable text for an arbitrary thrown value, for a tagged error's
 * composed `message`. `String(e)` degrades to `"[object Object]"` on the
 * non-`Error` values `execFileSync` and `fetch` can reject with.
 *
 * @param {unknown} cause
 * @returns {string}
 */
export function describe(cause) {
  if (typeof cause === "string") return cause;
  if (cause instanceof Error) return cause.message;
  if (cause === null || cause === undefined) return "unknown error";
  try {
    return JSON.stringify(cause);
  } catch {
    return String(cause);
  }
}
