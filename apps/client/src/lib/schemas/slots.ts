import { z } from "zod";

/** Runtime validators for the new-slot flow (`components/inline-new-slot.tsx`):
 * `slot_create`'s result and `slot_base_branches`' list — both feed straight
 * into a branch name and a `tt slot` invocation, so a malformed payload is
 * worth catching before it's acted on (#38). */

export const SlotCreatedSchema = z.object({
  name: z.string(),
  dir: z.string(),
  branch: z.string(),
  base: z.string(),
  /** The ref the slot effectively branched from — `origin/<base>` when the
   * creation-time fast-forward applied, else `base`. What the dynamic-flow
   * prompt names as its rebase target. */
  baseLabel: z.string(),
  warnings: z.array(z.string()),
});

/** One base choice from `slot_base_branches`: `name` is the local branch the
 * form submits as the base; `label` is the ref creation will effectively
 * branch from (`origin/main` for a default branch with a remote counterpart —
 * `slot_create` fetches and fast-forwards it first), shown instead of `name`
 * so the form doesn't undersell what actually happens. Deliberately the
 * opposite of `comparedBaseLabel` (lib/agentboard.ts), which *strips*
 * `origin/` — there the local/origin distinction is noise, here it's the
 * message. Don't "unify" them. */
export const BaseBranchesSchema = z.array(
  z.object({
    name: z.string(),
    label: z.string(),
  }),
);

export type BaseBranch = z.infer<typeof BaseBranchesSchema>[number];

/** `slot_write_pasted_images`' result: the absolute path of each staged
 * image, in paste order. These go straight into Claude's opening prompt, so a
 * malformed payload is worth catching here rather than as a path that
 * silently fails to read inside the session. */
export const PastedImagePathsSchema = z.array(z.string());

/** One reason `slot_remove` refused. The strongest case in this file for
 * validating: `port` is handed straight to `slot_stop_port`, which SIGTERMs
 * (then SIGKILLs) a process group — a payload that isn't the shape we think
 * it is should fail as a typed `SchemaMismatch` before anything gets
 * signaled, not after.
 *
 * `kind` stays an open `string`, not an enum: it crosses an IPC boundary
 * where an older frontend can meet a backend that grew a new guard, and the
 * UI is built to render an unknown kind generically (see `BlockerIcon`).
 * Rejecting the whole payload over one unrecognized discriminant would turn
 * a handled case into a hard failure. */
export const SlotBlockerSchema = z.object({
  kind: z.string(),
  message: z.string(),
  remedy: z.string(),
  losesWork: z.boolean(),
  port: z.number().int().positive().max(65535).nullish(),
});

/** `slot_remove`'s result — removed, or refused with reasons. A tagged union
 * on `status`, mirroring `SlotRemoveOutcome` in
 * `crates-tauri/tt-app/src/slots.rs`. */
export const SlotRemoveOutcomeSchema = z.discriminatedUnion("status", [
  z.object({
    status: z.literal("removed"),
    name: z.string(),
    messages: z.array(z.string()),
  }),
  z.object({
    status: z.literal("blocked"),
    name: z.string(),
    blockers: z.array(SlotBlockerSchema),
    /** Caveats gathered before the verdict — in practice a failed
     * `fetch --prune`, meaning the guards judged against stale `origin/*`.
     * Rendered above the blockers so an offline refusal doesn't read as an
     * authoritative one. */
    messages: z.array(z.string()),
  }),
]);
