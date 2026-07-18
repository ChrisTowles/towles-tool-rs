/**
 * Enter/exit motion for agentboard rail rows (repo, folder, session).
 *
 * The rail renders an immutable snapshot pushed from the backend
 * (`agentboard://state`): an untracked repo, a deleted slot, or a closed
 * session is simply *absent* from the next payload, so React would unmount its
 * row before anything could animate. Wrapping each level's `.map()` in
 * `<AnimatePresence initial={false}>` and each row in a `<motion.div>` spreading
 * this config keeps a departed row mounted through its exit.
 *
 * Structure follows yaak's AnimatePresence + `initial/animate/exit` pair. Note
 * that yaak itself does *not* animate its sidebar tree — this applies their
 * overlay idiom to a list, which is why `layout` matters here and doesn't
 * there.
 */

/**
 * Spread onto the `motion.div` wrapping one rail row.
 *
 * `layout` is the whole point of using motion here rather than CSS keyframes:
 * it transform-animates the *surviving* rows into their new positions, so the
 * rows below a departing one slide up instead of snapping. Collapsing the
 * leaver's own height is what frees that space, and `overflow: hidden` is
 * scoped to `exit` deliberately — applied at rest it would make the row a
 * scroll container for the `sticky` repo headers nested inside it and break
 * them. A row on its way out has no sticky behavior left to protect.
 *
 * `layout="position"`, not `layout: true`: the plain form animates a box's
 * *size* with scaleY, which visibly squashes the branch names and diff stats
 * inside a row whose height is changing (measured ~4% mid-animation). Position
 * is all a rail row needs — it moves, it never resizes.
 *
 * The duration lives here rather than in the app-level `MotionConfig`: it is
 * tuning for this animation, not a policy every future motion component in the
 * app should silently inherit. `MotionConfig` carries only `reducedMotion`.
 */
export const railRowMotion = {
  layout: "position",
  initial: { opacity: 0, x: -8 },
  animate: { opacity: 1, x: 0 },
  exit: { opacity: 0, x: -8, height: 0, overflow: "hidden" },
  transition: { duration: 0.15 },
} as const;
