import { useEffect, useRef, useSyncExternalStore } from "react";
import type { ScreenId } from "./screens";

/**
 * Deep-link focus primitive. The app's only nav verb is `openTab(screen)`,
 * which lands on a screen but never on a particular row. A {@link FocusTarget}
 * carries "and reveal *this* row once you're there": the destination screen
 * scrolls the matching row into view and flashes a highlight ring, then the
 * target clears (it's a one-shot, not a persistent selection).
 *
 * The store is a DOM-free singleton so it unit-tests cleanly and dodges an
 * import cycle: `workspace.tsx` writes to it from `openTabWithFocus` (via the
 * exported {@link focusTargetStore}), and destination screens read it through
 * {@link useFocusTarget} — neither has to import the other.
 */

export type FocusKind = "pr" | "todo" | "repo" | "issue";

/** "Land on `screen` and reveal the row identified by `kind`/`id`." */
export type FocusTarget = { screen: ScreenId; kind: FocusKind; id: string };

/**
 * The one transient focus request in flight, with set/consume/clear semantics.
 * `consume(screen)` is a one-shot read scoped to a screen: it returns (and
 * clears) the target only when it addresses that screen, so a non-matching
 * screen's read leaves the request in place for its real destination.
 */
export class FocusTargetStore {
  private target: FocusTarget | null = null;
  private listeners = new Set<() => void>();

  /** Snapshot for `useSyncExternalStore` — stable reference until it changes. */
  get = (): FocusTarget | null => this.target;

  set(target: FocusTarget): void {
    this.target = target;
    this.emit();
  }

  /** Return the pending target iff it addresses `screen`, clearing it. A
   * request for another screen is left untouched. */
  consume(screen: ScreenId): FocusTarget | null {
    if (this.target === null || this.target.screen !== screen) return null;
    const consumed = this.target;
    this.target = null;
    this.emit();
    return consumed;
  }

  clear(): void {
    if (this.target === null) return;
    this.target = null;
    this.emit();
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => {
      this.listeners.delete(fn);
    };
  };

  private emit(): void {
    for (const fn of [...this.listeners]) fn();
  }
}

/** App-wide singleton: at most one focus request pending at a time. */
export const focusTargetStore = new FocusTargetStore();

/** Flash-ring classes toggled on the matched row (paired dark variant); listed
 * as literals so Tailwind's scanner keeps them in the build. */
const FLASH_CLASSES = ["ring-2", "ring-inset", "ring-amber-400", "dark:ring-amber-500"];
const FLASH_MS = 1600;
/** The target row may not be painted yet (snapshot still loading) — retry a
 * few frames before giving up. */
const MAX_ATTEMPTS = 12;
const RETRY_MS = 150;

/** The row carrying `data-focus-kind`/`data-focus-id` within `container`. Ids
 * can hold `/`, `#`, path separators, so match on the dataset rather than
 * building a fragile attribute selector. */
export function findFocusRow(
  container: HTMLElement,
  kind: FocusKind,
  id: string,
): HTMLElement | null {
  const rows = container.querySelectorAll<HTMLElement>(`[data-focus-kind="${kind}"]`);
  for (const row of rows) {
    if (row.dataset.focusId === id) return row;
  }
  return null;
}

function flashRow(el: HTMLElement): void {
  el.scrollIntoView({ block: "center", behavior: "smooth" });
  el.classList.add(...FLASH_CLASSES);
  window.setTimeout(() => el.classList.remove(...FLASH_CLASSES), FLASH_MS);
}

/**
 * Wire a destination screen to the focus primitive: attach the returned ref to
 * the scroll container that holds the focusable rows (each tagged with
 * `data-focus-kind` + `data-focus-id`). When a target for `screen` arrives it
 * is consumed once, the matching row scrolls into view and flashes, and the
 * request clears. Scoping the row lookup to the container keeps a still-mounted
 * background screen's identical row from being matched by mistake.
 */
export function useFocusTarget<T extends HTMLElement>(screen: ScreenId) {
  const containerRef = useRef<T>(null);
  const focusTarget = useSyncExternalStore(
    focusTargetStore.subscribe,
    focusTargetStore.get,
    focusTargetStore.get,
  );

  useEffect(() => {
    if (!focusTarget || focusTarget.screen !== screen) return;
    const target = focusTargetStore.consume(screen);
    if (!target) return;

    let attempts = 0;
    let timer = 0;
    const attempt = () => {
      const container = containerRef.current;
      const el = container ? findFocusRow(container, target.kind, target.id) : null;
      if (el) {
        flashRow(el);
        return;
      }
      if (++attempts < MAX_ATTEMPTS) timer = window.setTimeout(attempt, RETRY_MS);
    };
    attempt();

    return () => {
      if (timer) window.clearTimeout(timer);
    };
  }, [focusTarget, screen]);

  return containerRef;
}
