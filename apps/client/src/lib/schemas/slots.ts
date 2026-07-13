import { z } from "zod";

/** Runtime validators for the new-slot flow (`components/new-slot-dialog.tsx`):
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
