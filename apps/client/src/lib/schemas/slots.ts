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
  warnings: z.array(z.string()),
});

export const BaseBranchesSchema = z.array(z.string());

/** `slot_write_pasted_images`' result: the absolute path of each staged
 * image, in paste order. These go straight into Claude's opening prompt, so a
 * malformed payload is worth catching here rather than as a path that
 * silently fails to read inside the session. */
export const PastedImagePathsSchema = z.array(z.string());
