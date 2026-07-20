/**
 * The one slug rule the frontend uses, mirroring `tt-git`'s branch-name
 * slugging on the Rust side.
 *
 * Shared rather than re-derived per caller: the slot flow turns a goal into a
 * branch name with it, and the calendar-source editor turns a label into a
 * **store-lane id** — a value that keys rows in `events` and can never be
 * changed afterwards without orphaning them. Two nearly-identical regexes
 * produced two different answers for the same label (a trailing `-`, `_`
 * handled differently), which is a cosmetic difference for a branch and a
 * permanent one for a lane.
 */
export function slugify(text: string): string {
  let slug = text.toLowerCase().trim().replaceAll(" ", "-");
  slug = slug.replace(/[^0-9a-z_-]/g, "-");
  slug = slug.replace(/-+/g, "-");
  slug = slug.replace(/-+$/, "");
  return slug;
}
