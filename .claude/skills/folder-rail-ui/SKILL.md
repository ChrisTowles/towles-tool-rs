---
name: folder-rail-ui
description: Visual design language for the Towles Tool desktop app (apps/client) — the "Folder Rail" style. Covers color tokens, agent-status semantics, the folder→session hierarchy, terminal panels, spacing, glyphs, and copy-paste Tailwind class recipes. Use when building or restyling ANY screen or component under apps/client (Agentboard, Cockpit, Board, dialogs, sidebar), or when the user mentions the "folder rail", "session rail", status dots/colors, or the app's look and feel.
user-invocable: true
---

# Folder Rail — the app's visual language

The Towles Tool desktop app is styled in one voice: a **neutral grayscale
shadcn base** (from `apps/client/src/index.css`, all `oklch(… 0 0)`) with a thin
**semantic color layer** on top for agent status and attention. This skill is
that layer, formalized. Match it when you touch anything under `apps/client`.

**Open the reference:** `assets/folder-rail-reference.html` — a live rendering
of the canonical layout + a palette swatch strip. Open it in a browser before
styling something new (`xdg-open` the file).

The mental model everything serves: **a repo is checked out into 1..N folders
(clone / worktree / slot); a folder holds 1..N sessions; a session is either a
`✦` Claude agent or a `❯` zsh shell.** Repo → Folder → Session. When a repo has
a single checkout the repo+folder collapse into one header so solo repos stay
clean. Attention bubbles *up* from session → folder → repo. Never flatten that
hierarchy away.

**Level markers (structure = gray icon, per §1):** a repo header leads with a
`FolderGit2` glyph + `font-semibold text-foreground` name; a folder (checkout)
sub-header is indented and leads with a plain `Folder` glyph (dimmer,
`text-muted-foreground/70`) + a `text-muted-foreground` name. This is how a
solo repo reads as a *repo*, not just another folder. Icons are
`size-3.5 text-muted-foreground`.

---

## 1. Color = meaning, never decoration

Neutral grays carry *structure*; a hue only ever appears to carry *state*. If
something is not conveying agent status or an attention signal, it is gray.

### Neutral shell — use shadcn tokens, not raw colors

| Role | Tailwind class | Notes |
|---|---|---|
| App/rail background | `bg-background` | the canvas |
| Card / sticky headers / terminal chrome | `bg-card` | folder headers, tab bars |
| Row hover | `hover:bg-accent/50` | |
| Active/selected row | `bg-accent` | |
| Hairline divider | `border-border` | 1px, everywhere |
| Primary text | `text-foreground` | |
| Secondary text | `text-muted-foreground` | branch names, timestamps, messages |
| Faint text | `text-muted-foreground/60` | glyphs, counts |

Radius: shadcn default `--radius: 0.625rem`. Cards/panels `rounded-lg`,
chips/badges/tabs `rounded-md`, dots are circles. Terminal panels `rounded-lg`.

### Status dots — mirror `statusColor()` exactly

Defined once in `apps/client/src/lib/agentboard.ts`; do **not** re-invent the
mapping. A dot is `size-2 rounded-full` (8px).

| `AgentStatus` | class | reads as |
|---|---|---|
| `busy` | `bg-yellow-500` | working |
| `waiting` | `bg-blue-500` | needs your input |
| `error` | `bg-red-500` | broke |
| `complete` | `bg-green-500` | done |
| `interrupted` | `bg-orange-500` | paused |
| `idle` (default) | `bg-muted-foreground/40` | quiet shell |

A busy/live dot may `animate-pulse`. Keep pulse for *active work only* — a
resting board should be still.

### Attention accent — amber

The "needs you" signal is **amber-500**, distinct from the yellow busy dot.
Used as a **left border** on a session/feed row and as a small count badge on
its folder. Mirrors `KIND_BORDER` in `agentboard.tsx`.

- Needs-you row: `border-l-2 border-l-amber-500`
- PR-failing feed row: `border-l-red-500` · calendar feed row: `border-l-blue-500`
- Folder count badge: amber text on faint amber wash — `text-amber-500
  border border-amber-500/50 bg-amber-500/10`

`needsYou = sessions where status ∈ {waiting, error}` (+ failing PRs). That set,
and only that set, gets the amber treatment.

### Agent / active accent — violet

The one hue net-new to this design. **violet-500** marks *agent-ness* and the
*currently focused* thing. It aligns with the app's existing dark
`--sidebar-primary` (a violet).

- Agent glyph `✦` → `text-violet-500`; shell glyph `❯` → `text-muted-foreground`
- Active session row: `border-l-2 border-l-violet-500 bg-accent`
- "+ session" / "+ agent" affordances: `text-violet-500`
- Terminal prompt `❯` caret: violet

> An active row that *also* needs you: **amber wins** the left border (attention
> outranks focus). Show violet elsewhere (glyph, tab).

---

## 2. Typography

- **Sans:** Geist Variable (`--font-sans`) — all UI chrome. Base 13px.
- **Mono:** `font-mono` for anything that is *data the terminal/git owns* —
  branch names, `+/−` diff stats, timestamps/ages, session counts, keyboard
  hints, the `✦`/`❯` glyphs, and terminal content.
- Folder name: `font-semibold`. Session name: normal weight `text-foreground`.
  Messages/branch/time: `text-muted-foreground`, ~11px.
- Diff stats: `text-green-500` `+N` and `text-red-500` `−N`, mono, small.

Rule of thumb: **if git or the shell produced the string, render it mono.**

---

## 3. The Folder Rail layout

Two panes. Left = navigation (the rail). Right = the focused terminal(s).

```
┌ rail (w-80) ─────────────┐┌ main (flex-1) ───────────────────┐
│ ▾ w/acme-web    ⎇ feat…   ││ w/acme-billing  ⎇ fix/webhook…    │
│   ✦●busy checkout-ui     ││ [✦ webhook-fix ●] [❯ shell 1 ●] +  │
│   ✦●error test-writer  ⚑ ││ ┌ terminal ──────────────────────┐│
│   ❯●busy shell 1         ││ │ ❯ …                            ││
│ ▾ w/acme-billing 1⚑ ⎇fix…││ │                                ││
│   ✦●waiting webhook-fix ⚑││ │                                ││
│   ❯○idle shell 1         ││ └────────────────────────────────┘│
└──────────────────────────┘└───────────────────────────────────┘
```

**Rail (`w-80`, `overflow-y-auto`):** a list of folders. Each folder:
- **Sticky header** (`sticky top-0 bg-card`, `border-b border-border`): a caret
  `▾`, the folder name (with a muted `w/` `p/` scope prefix in mono), then a
  right-aligned meta cluster — amber count badge (only if it has needs-you
  sessions) + branch name in mono.
- **Session rows** (indented ~30px under the header): `glyph · status-dot ·
  name · right-aligned status message`, with a trailing amber `attn` micro-dot
  when it needs you. Row is `border-l-2` transparent by default → violet when
  active → amber when needs-you.

**Main pane:** header (`bg-card border-b`) with the folder's `scope+name` +
branch, then a row of **session tabs** (`✦`/`❯` glyph + name + status dot;
active tab `bg-accent`), a `+ session` in violet, and right-aligned keyboard
affordances (`Split ⌘D`, `Close ⌘W`). Below, the terminal panel(s) in a
`p-3.5` wrap. Split panes stay mounted and toggle with `hidden` so scrollback
survives (existing Agentboard behavior — preserve it).

**Terminal panel:** near-black `#07090c` inside the neutral shell, `rounded-lg
border border-border`, `font-mono text-xs leading-relaxed`, `p-3`. Prompt caret
`❯` is violet.

Spacing rhythm: rail rows `px-3 py-1.5`; headers `px-3 py-2.5`; gaps `gap-2`
between glyph/dot/name; panel padding `p-3.5`.

---

## 4. Component recipes (copy-paste)

**Status dot**
```tsx
<span className={cn("size-2 rounded-full", statusColor(status), live && "animate-pulse")} />
```

**Session row**
```tsx
<button className={cn(
  "flex items-center gap-2.5 py-1.5 pr-3 pl-7 text-left border-l-2 border-transparent",
  "hover:bg-accent/50",
  active && "bg-accent border-l-violet-500",
  needsYou && "border-l-amber-500",
)}>
  <span className={cn("font-mono text-xs w-4 text-center",
    type === "agent" ? "text-violet-500" : "text-muted-foreground")}>
    {type === "agent" ? "✦" : "❯"}
  </span>
  <span className={cn("size-2 rounded-full", statusColor(status))} />
  <span className="text-foreground">{name}</span>
  <span className="ml-auto text-[11px] text-muted-foreground">{message}</span>
  {needsYou && <span className="size-1.5 rounded-full bg-amber-500" />}
</button>
```

**Folder count badge (needs-you)**
```tsx
<span className="rounded-md border border-amber-500/50 bg-amber-500/10 px-1.5 font-mono text-[10.5px] text-amber-500">
  {n} ⚑
</span>
```

**Branch + diff stats**
```tsx
<span className="font-mono text-[11px] text-muted-foreground">⎇ {branch}</span>
<span className="font-mono text-[11px] text-green-500">+{add}</span>
<span className="font-mono text-[11px] text-red-500">−{del}</span>
```

---

## 5. Do / Don't

- **Do** drive every hue from `statusColor()` / the amber+violet accents above.
  If you need a new color, you probably need a new *status* — question it.
- **Do** keep folder → session hierarchy explicit. Sessions are grouped by
  folder, never a flat undifferentiated list.
- **Do** distinguish agent vs shell with the `✦`/`❯` glyph + violet, so a glance
  tells them apart.
- **Do** use shadcn tokens (`bg-card`, `border-border`, `text-muted-foreground`)
  so light + dark both work. The reference HTML is dark-only for brevity; real
  components must render in both (`.dark` variant handled by tokens).
- **Don't** hand-write CSS, CSS modules, or CSS-in-JS (project rule) — Tailwind
  utilities + shadcn only. Add primitives with `npx shadcn@latest add <name>`.
- **Don't** color things for decoration. Gray is the default; a hue is a claim
  about state.
- **Don't** animate resting UI. `animate-pulse` is reserved for a live/busy dot.
- **Don't** let focus (violet) override attention (amber) on a row's left border.

## Reference files in the app
- `apps/client/src/lib/agentboard.ts` — `statusColor()`, `AgentStatus`, session/agent types.
- `apps/client/src/screens/agentboard.tsx` — `KIND_BORDER`, rail + split-terminal layout.
- `apps/client/src/components/day-bar.tsx` — the needs-you attention math.
- `apps/client/src/index.css` — shadcn token definitions (light/dark).
