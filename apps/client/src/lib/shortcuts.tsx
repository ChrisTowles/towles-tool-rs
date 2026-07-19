import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { SETTINGS_SAVED_EVENT, loadUserSettings } from "./settings";

/**
 * Data-driven keyboard shortcuts (modeled on plannotator's validated registry,
 * `packages/ui/shortcuts/core.ts`): every binding is declared once with a
 * scope, a parsed-and-validated key spec, and a description — the same data
 * drives matching, the on-screen hints (`shortcutKeys`), and the `?` help
 * overlay. Adding a shortcut = adding a `defineShortcuts` entry; nothing is
 * matched from ad-hoc `e.key === "d"` checks anymore.
 *
 * Guards: shortcuts never fire from editable targets — inputs, textareas,
 * contenteditable, or anything inside a `[data-term-host]` terminal (Ctrl+D at
 * a shell prompt is EOF, not "new session"). A binding opts out of the guard
 * with `allowInEditable` — used for agentboard actions that operate on board
 * state rather than the focused element (e.g. "jump to next session needing
 * you"), so they work even while a terminal has focus. `matchesEditableOverride`
 * lets the terminal itself recognize these and yield the keystroke instead of
 * sending it to the shell. The whole opt-out is gated behind the
 * `agentboard.shortcutsWorkInTerminal` setting (default on; see
 * `useShortcutsWorkInTerminal`) so a user can restore the old
 * terminal-owns-everything behavior.
 */

export type ShortcutScope = "global" | "agentboard" | "board";

/** A parsed key spec: modifiers + one main key. `mod` = ⌘ on mac, Ctrl elsewhere. */
type KeySpec = {
  mod: boolean;
  shift: boolean;
  alt: boolean;
  /** `KeyboardEvent.key`, lowercased ("d", ",", "?"). */
  key: string;
};

export type Shortcut = {
  id: string;
  scope: ShortcutScope;
  /** Human-readable spec, validated at definition time: "mod+shift+w", "?". */
  keys: string;
  /** What it does — shown in the help overlay. */
  description: string;
  /** Shown muted in the overlay when the action needs context (e.g. "when a
   * session is selected"). */
  when?: string;
  allowInEditable?: boolean;
  /** Registered + matched like any other, but omitted from the `?` overlay —
   * used to collapse a run of near-identical bindings (mod+1…mod+9) to one line. */
  hideInHelp?: boolean;
  spec: KeySpec;
};

const MODIFIER_TOKENS = new Set(["mod", "shift", "alt"]);

/** Parse + validate a key spec. Throws at module-eval time on a bad spec, so a
 * typo'd binding fails the build/tests instead of silently never matching. */
function parseKeys(keys: string): KeySpec {
  const tokens = keys.toLowerCase().split("+");
  const spec: KeySpec = { mod: false, shift: false, alt: false, key: "" };
  for (const t of tokens) {
    if (MODIFIER_TOKENS.has(t)) {
      spec[t as "mod" | "shift" | "alt"] = true;
    } else if (t.length > 0 && !spec.key) {
      spec.key = t;
    } else {
      throw new Error(`Invalid shortcut spec "${keys}": bad token "${t}"`);
    }
  }
  if (!spec.key) throw new Error(`Invalid shortcut spec "${keys}": no main key`);
  return spec;
}

function defineShortcuts(defs: Omit<Shortcut, "spec">[]): Record<string, Shortcut> {
  const out: Record<string, Shortcut> = {};
  for (const d of defs) {
    if (out[d.id]) throw new Error(`Duplicate shortcut id "${d.id}"`);
    out[d.id] = { ...d, spec: parseKeys(d.keys) };
  }
  return out;
}

/** The one registry. Scopes: `global` is always active; `agentboard` only
 * while the Agentboard tab is shown (its screen stays mounted when hidden —
 * without the scope gate its bindings would fire from every other tab). */
export const SHORTCUTS = defineShortcuts([
  { id: "palette", scope: "global", keys: "mod+k", description: "Command palette" },
  { id: "settings", scope: "global", keys: "mod+,", description: "Settings" },
  { id: "sidebar", scope: "global", keys: "mod+b", description: "Collapse sidebar to icons" },
  { id: "quicklog", scope: "global", keys: "mod+j", description: "Quick journal log" },
  {
    id: "zen",
    scope: "global",
    keys: "mod+shift+f",
    description: "Zen focus mode — hide chrome (Escape exits)",
  },
  {
    id: "close-tab",
    scope: "global",
    keys: "mod+w",
    description: "Close the current tab",
    when: "more than one tab is open",
  },
  // Jump to the Nth open tab. One binding per digit so each is validated and
  // matched through the same machinery as every other shortcut; the `?` help
  // overlay is the only place they're surfaced, so it lists just the first.
  ...Array.from({ length: 9 }, (_, i) => ({
    id: `tab-${i + 1}`,
    scope: "global" as const,
    keys: `mod+${i + 1}`,
    description: i === 0 ? "Jump to tab 1–9" : `Jump to tab ${i + 1}`,
    hideInHelp: i > 0,
  })),
  {
    id: "next-tab",
    scope: "global",
    keys: "mod+]",
    description: "Cycle to the next tab",
    when: "more than one tab is open",
  },
  {
    id: "prev-tab",
    scope: "global",
    keys: "mod+[",
    description: "Cycle to the previous tab",
    when: "more than one tab is open",
  },
  { id: "help", scope: "global", keys: "?", description: "Keyboard shortcuts (this overlay)" },
  {
    id: "board-filter",
    scope: "board",
    keys: "/",
    description: "Focus the todo filter",
  },
  {
    id: "ab-new-session",
    scope: "agentboard",
    keys: "mod+d",
    description: "New session in the focused folder",
    when: "a folder is focused",
  },
  {
    id: "ab-new-slot",
    scope: "agentboard",
    keys: "mod+shift+d",
    description: "New task — goal, issues, branch",
    when: "a folder is focused",
    allowInEditable: true,
  },
  {
    id: "ab-remove-slot",
    scope: "agentboard",
    keys: "mod+shift+backspace",
    description: "Delete the focused worktree slot (confirms first)",
    when: "a worktree slot is focused",
    allowInEditable: true,
  },
  {
    id: "ab-close-session",
    scope: "agentboard",
    keys: "mod+shift+w",
    description: "Close the selected session (kills its shell)",
    when: "a session is selected",
    allowInEditable: true,
  },
  {
    id: "ab-toggle-diff",
    scope: "agentboard",
    keys: "mod+shift+g",
    description: "Open the focused folder's diff pane",
    when: "a folder is focused",
    allowInEditable: true,
  },
  {
    id: "ab-toggle-files",
    scope: "agentboard",
    keys: "mod+shift+e",
    description: "Open the focused folder's files pane",
    when: "a folder is focused",
    allowInEditable: true,
  },
  {
    id: "ab-toggle-preview",
    scope: "agentboard",
    keys: "mod+shift+v",
    description: "Open the focused folder's live-preview pane",
    when: "a folder is focused",
    allowInEditable: true,
  },
  {
    id: "ab-toggle-rail",
    scope: "agentboard",
    keys: "mod+shift+b",
    description: "Collapse the folder rail to icons (and back)",
    allowInEditable: true,
  },
  {
    id: "ab-jump-next",
    scope: "agentboard",
    keys: "mod+shift+n",
    description: "Jump to next session needing you",
    allowInEditable: true,
  },
  {
    id: "ab-jump-prev",
    scope: "agentboard",
    keys: "mod+shift+p",
    description: "Jump to previous session needing you",
    allowInEditable: true,
  },
  {
    id: "ab-split-session",
    scope: "agentboard",
    keys: "mod+shift+s",
    description: "Add another session as a pane in this window",
    when: "a folder is focused",
    allowInEditable: true,
  },
  {
    id: "ab-new-terminal-right",
    scope: "agentboard",
    keys: "mod+shift+o",
    description: "Open a new terminal to the right",
    when: "a folder is focused",
    allowInEditable: true,
  },
  {
    // Handled by the focused TerminalView itself (via `matchesShortcut`), not
    // a window-level handler: only the terminal that owns the keystroke may
    // open its overlay. Ctrl+F stays with the shell; the shifted chord is ours.
    id: "term-search",
    scope: "agentboard",
    keys: "mod+shift+f",
    description: "Search terminal scrollback",
    when: "a terminal is focused",
  },
]);

/** True on macOS — chooses ⌘ vs Ctrl for the modifier key across the app. */
export const IS_MAC = typeof navigator !== "undefined" && /mac/i.test(navigator.platform ?? "");

/** Keycap symbols for multi-char `KeyboardEvent.key` names that would
 * otherwise render as raw lowercase words in the help overlay. */
const KEYCAP_LABELS: Record<string, string> = { backspace: "⌫" };

/** Per-platform keycap tokens for a shortcut id: ["⌘","⇧","W"] on mac,
 * ["Ctrl","Shift","W"] elsewhere. Feed to <Kbd> or join for a title. */
export function shortcutKeys(id: string): string[] {
  const s = SHORTCUTS[id];
  if (!s) throw new Error(`Unknown shortcut id "${id}"`);
  const caps: string[] = [];
  if (s.spec.mod) caps.push(IS_MAC ? "⌘" : "Ctrl");
  if (s.spec.shift) caps.push(IS_MAC ? "⇧" : "Shift");
  if (s.spec.alt) caps.push(IS_MAC ? "⌥" : "Alt");
  caps.push(
    KEYCAP_LABELS[s.spec.key] ?? (s.spec.key.length === 1 ? s.spec.key.toUpperCase() : s.spec.key),
  );
  return caps;
}

/** One string for tooltips/titles: "⌘⇧W" on mac, "Ctrl+Shift+W" elsewhere. */
export function shortcutHint(id: string): string {
  return shortcutKeys(id).join(IS_MAC ? "" : "+");
}

/** Whether a keydown matches a registry shortcut — for components that own
 * their keystrokes (the terminal view) and match locally instead of through
 * the window-level `useShortcuts` listener. */
export function matchesShortcut(id: string, e: KeyboardEvent): boolean {
  const s = SHORTCUTS[id];
  if (!s) throw new Error(`Unknown shortcut id "${id}"`);
  return matches(s.spec, e);
}

/** True when a keydown matches a shortcut that opts out of the editable-target
 * guard (`allowInEditable`) — lets a component that owns its own keydown
 * handling (the terminal) recognize a board-wide action and yield the
 * keystroke to the window-level listener instead of consuming it (e.g.
 * sending it to the shell as a control byte). */
export function matchesEditableOverride(e: KeyboardEvent): boolean {
  for (const s of Object.values(SHORTCUTS)) {
    if (s.allowInEditable && matches(s.spec, e)) return true;
  }
  return false;
}

/** Built-in default for `agentboard.shortcutsWorkInTerminal` — on, matching
 * tt-config. */
export const DEFAULT_SHORTCUTS_WORK_IN_TERMINAL = true;

/**
 * Track the `agentboard.shortcutsWorkInTerminal` preference in a ref so the
 * terminal's keydown handler and the window-level shortcut listener can read
 * it live without re-subscribing. Re-reads on `SETTINGS_SAVED_EVENT` (fired
 * right after a successful save, wherever Settings is edited — see
 * `useUserSettings` in `settings.ts`) and on window focus (covers the JSON
 * file being edited externally then alt-tabbing back), matching
 * {@link useCopyOnSelect} in `terminal-prefs.ts`.
 */
export function useShortcutsWorkInTerminal(): RefObject<boolean> {
  const ref = useRef(DEFAULT_SHORTCUTS_WORK_IN_TERMINAL);
  useEffect(() => {
    let alive = true;
    const load = () =>
      void loadUserSettings().then((s) => {
        if (alive && s)
          ref.current = s.agentboard?.shortcutsWorkInTerminal ?? DEFAULT_SHORTCUTS_WORK_IN_TERMINAL;
      });
    load();
    window.addEventListener("focus", load);
    window.addEventListener(SETTINGS_SAVED_EVENT, load);
    return () => {
      alive = false;
      window.removeEventListener("focus", load);
      window.removeEventListener(SETTINGS_SAVED_EVENT, load);
    };
  }, []);
  return ref;
}

function matches(spec: KeySpec, e: KeyboardEvent): boolean {
  const mod = IS_MAC ? e.metaKey : e.ctrlKey;
  // `?` arrives as key "?" with shiftKey set — compare shift only for
  // modifier-style specs, where shift is a deliberate chord component.
  const shiftOk = spec.mod ? e.shiftKey === spec.shift : true;
  return mod === spec.mod && shiftOk && e.altKey === spec.alt && e.key.toLowerCase() === spec.key;
}

/** True when the event originated somewhere that owns its own keystrokes: an
 * input/textarea/contenteditable, or a terminal (`[data-term-host]`) — where
 * e.g. Ctrl+D is EOF and every printable char belongs to the shell. */
function isEditableTarget(e: KeyboardEvent): boolean {
  const el = e.target;
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  const tag = el.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return el.closest("[data-term-host]") != null;
}

/**
 * Bind handlers for registry shortcuts. Handlers are keyed by shortcut id; a
 * matched binding runs its handler and eats the event. `enabled` gates the
 * whole set (scope activation — e.g. Agentboard passes `activeTab ===
 * "agentboard"` because its screen stays mounted while hidden).
 */
export function useShortcuts(handlers: Partial<Record<string, () => void>>, enabled = true): void {
  const workInTerminalRef = useShortcutsWorkInTerminal();
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (e: KeyboardEvent) => {
      for (const [id, handler] of Object.entries(handlers)) {
        if (!handler) continue;
        const s = SHORTCUTS[id];
        if (!s) throw new Error(`Unknown shortcut id "${id}"`);
        if (!matches(s.spec, e)) continue;
        const allowed = s.allowInEditable && workInTerminalRef.current;
        if (isEditableTarget(e) && !allowed) return;
        e.preventDefault();
        handler();
        return;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handlers, enabled, workInTerminalRef]);
}

const SCOPE_TITLES: Record<ShortcutScope, string> = {
  global: "Everywhere",
  agentboard: "Agentboard",
  board: "Board",
};

/**
 * The `?` help overlay: every registered shortcut, grouped by scope, with
 * platform keycaps. Scopes not currently active render muted so the sheet
 * doubles as discovery ("Agentboard has ⌘D" even while you're on Cockpit).
 */
export function ShortcutHelp({
  open,
  onOpenChange,
  activeScopes,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  activeScopes: ShortcutScope[];
}) {
  const byScope = useMemo(() => {
    const m = new Map<ShortcutScope, Shortcut[]>();
    for (const s of Object.values(SHORTCUTS)) {
      if (s.hideInHelp) continue;
      const list = m.get(s.scope) ?? [];
      list.push(s);
      m.set(s.scope, list);
    }
    return m;
  }, []);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          {[...byScope.entries()].map(([scope, shortcuts]) => {
            const active = activeScopes.includes(scope);
            return (
              <div key={scope} className={active ? undefined : "opacity-50"}>
                <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  {SCOPE_TITLES[scope]}
                  {!active && " — inactive here"}
                </div>
                <div className="flex flex-col gap-1">
                  {shortcuts.map((s) => (
                    <div key={s.id} className="flex items-baseline gap-3 text-sm">
                      <KbdGroup className="w-28 shrink-0 justify-start">
                        {shortcutKeys(s.id).map((cap) => (
                          <Kbd key={cap}>{cap}</Kbd>
                        ))}
                      </KbdGroup>
                      <span className="min-w-0 flex-1">
                        {s.description}
                        {s.when && <span className="text-muted-foreground"> — {s.when}</span>}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Owns the `?` binding + the overlay's open state. Mounted once in App. */
export function ShortcutHelpHost({ activeScopes }: { activeScopes: ShortcutScope[] }) {
  const [open, setOpen] = useState(false);
  useShortcuts(useMemo(() => ({ help: () => setOpen(true) }), []));
  return <ShortcutHelp open={open} onOpenChange={setOpen} activeScopes={activeScopes} />;
}
