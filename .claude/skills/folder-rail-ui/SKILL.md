---
name: folder-rail-ui
description: Visual design language ("Folder Rail" style) for new or restyled UI in the Towles Tool desktop app (apps/client) — color tokens, agent-status semantics, folder/session hierarchy, spacing, glyphs, Tailwind recipes. Use when adding a new screen/component, restyling an existing one, or the user asks about the app's look, the "folder rail", status dots/colors, or the repo→folder→session hierarchy. Not needed for logic-only changes to already-styled components.
user-invocable: true
---

# Folder Rail — visual language cheat sheet

Neutral grayscale shadcn base (`apps/client/src/index.css`); a hue is added
only to carry agent status or attention, never decoration.

**Hierarchy:** repo (1..N folders: clone/worktree/task) → folder (1..N
sessions) → session (`✦` Claude agent or `❯` zsh shell). Solo-repo folders
collapse repo+folder into one header. Attention bubbles up session→folder→repo.
Never flatten this.

## Neutral tokens
| Role | Class |
|---|---|
| Background | `bg-background` |
| Card/header/terminal chrome | `bg-card` |
| Hover (row on `bg-card`) | `hover:bg-accent/50` |
| Hover (row on bare `bg-background`) | `bg-accent` (full — `/50` misreads as a header at dark-mode lightness) |
| Active/selected row | `bg-accent` |
| Divider | `border-border` |
| Primary / secondary / faint text | `text-foreground` / `text-muted-foreground` / `text-muted-foreground/60` |

Radius: cards/panels `rounded-lg`, chips/badges/tabs `rounded-md`, dots circular.

## Status dots — mirror `statusColor()` in `apps/client/src/lib/agentboard.ts` exactly
| `AgentStatus` | class |
|---|---|
| `busy` | `bg-cyan-500` (`animate-pulse` while live only) — deliberately *not* amber/yellow, which reads as the needs-you accent below |
| `waiting` | `bg-blue-500` |
| `error` | `bg-red-500` |
| `complete` | `bg-green-500` |
| `interrupted` | `bg-orange-800` — not orange-500, which sits inside amber-500's *and* red-500's confusion radius (OKLab ΔE ~10); an unseen interrupted session shows this dot inside an amber-washed needs-you row, so it must stay clearly not-amber |
| `idle` (default) | `bg-muted-foreground/40` |

Dot = `size-2 rounded-full`.

When adding or re-tuning a status/accent color, don't eyeball hue distance —
check OKLab ΔE by hand against the full set of colors that can appear
adjacent (status dots + amber + violet). A normal-vision ΔE under ~15 between
two colors that can co-occur is a real risk, not a nitpick — that's how the
original yellow/amber busy bug and the orange/amber interrupted bug both got in.

## Two accent hues, one rule each
- **Amber (`amber-500`)** = needs-you (`status ∈ {waiting, error}` + failing
  PRs). A needs-you *row* (session list) gets the full treatment: left border
  `border-l-2 border-l-amber-500`, a row-wide wash `bg-amber-500/10`
  (`/15` on hover), and a flag dot `bg-amber-500` sitting right after the
  status dot — in the same glance as the glyph and name, not stranded at the
  row's far edge. A thin border alone was tested and rejected: it reads as a
  decoration you have to go looking for, not an alert. A needs-you *badge*
  (folder/repo aggregate count, not a full row) stays the lighter chip:
  `text-amber-500 border border-amber-500/50 bg-amber-500/10`.
- **Violet (`violet-500`)** = agent-ness / currently focused. Agent glyph `✦`,
  active row `border-l-2 border-l-violet-500 bg-accent`, `+ session` action,
  terminal prompt caret.
- If a row is both active and needs-you: **amber wins the border and the
  fill**; show violet elsewhere (glyph, tab). Needs-you is the rarer, more
  urgent signal — "this is where you're currently looking" is redundant once
  you're looking at it.

## Level ladder (never let a deeper level outrank its parent)
| Level | Glyph | Weight | Indent |
|---|---|---|---|
| Repo | `FolderGit2`, `text-muted-foreground` | `font-semibold text-foreground` | `px-3` |
| Folder | `Folder`, `/70` | `font-medium text-muted-foreground` | `pl-6` |
| Session | `✦` violet / `❯` `/60` | normal (`text-foreground` if live) | `pl-9` + `ml-1.5` |

Every header row (repo, folder, or a solo-repo's collapsed header) gets
`border-b border-border` so it reads as a header band, not a session row.

## Typography
- Sans (Geist Variable) for UI chrome, 13px base.
- `font-mono` for anything git/shell-owned: branch names, `+/−` diff stats,
  timestamps, counts, keyboard hints, `✦`/`❯`, terminal content.
- Diff stats: `text-green-500 +N` / `text-red-500 −N`, mono.

## Component recipes
```tsx
// status dot
<span className={cn("size-2 rounded-full", statusColor(status), live && "animate-pulse")} />

// session row — a real <button> can't be used here: this row contains its
// own interactive descendants (inline rename input, trailing action
// buttons), which is invalid inside <button> and React only reports at
// runtime, not via lint/tsc/tests. Use a div with role="button" instead.
<div
  role="button"
  tabIndex={0}
  onClick={onSelect}
  onKeyDown={(e) => e.key === "Enter" && onSelect()}
  className={cn(
    "flex items-center gap-2.5 py-1.5 pr-3 pl-9 cursor-pointer border-l-2 border-transparent",
    hovered && !needsYou && "bg-accent",
    active && !needsYou && "bg-accent border-l-violet-500",
    // needs-you wins the edge AND the fill over hover/active — see "Two accent
    // hues" above for why a thin border alone isn't enough.
    needsYou && "border-l-amber-500 bg-amber-500/10",
    needsYou && hovered && "bg-amber-500/15",
  )}
>
  <span className={cn("font-mono text-xs w-4 text-center",
    type === "agent" ? "text-violet-500" : "text-muted-foreground/60")}>
    {type === "agent" ? "✦" : "❯"}
  </span>
  <span className={cn("size-2 rounded-full", statusColor(status))} />
  {needsYou && <span className="size-1.5 rounded-full bg-amber-500" />}
  <span className="text-foreground">{name}</span>
  <span className="ml-auto text-[11px] text-muted-foreground">{message}</span>
</div>

// folder count badge (needs-you)
<span className="rounded-md border border-amber-500/50 bg-amber-500/10 px-1.5 font-mono text-[10.5px] text-amber-500">
  {n} ⚑
</span>

// branch + diff stats
<span className="font-mono text-[11px] text-muted-foreground">⎇ {branch}</span>
<span className="font-mono text-[11px] text-green-500">+{add}</span>
<span className="font-mono text-[11px] text-red-500">−{del}</span>
```

## Do / Don't
- Do drive every hue from `statusColor()` or the amber/violet rules above —
  a new color implies a new *status*, question it.
- Do use shadcn tokens (not raw colors) so light + dark both work.
- Don't hand-write CSS/CSS-in-JS — Tailwind + shadcn only (`npx shadcn@latest add <name>`).
- Don't animate resting UI — `animate-pulse` is reserved for a live, currently-
  true nudge (the busy dot; the cache badge's "cold and worth compacting"
  pill), never a passive fact or a summary/rollup count.
- Don't let violet (focus) override amber (attention) on a row's border.

## Source of truth in the app
`apps/client/src/lib/agentboard.ts` (`statusColor()`, types) ·
`apps/client/src/components/agentboard-rail.tsx` (rail/split layout, row markup) ·
`apps/client/src/components/day-bar.tsx` (needs-you math) ·
`apps/client/src/index.css` (token definitions).

For behavior/flow rules (confirmations, error copy, when something needs a
setting) rather than look, see the `ui-rules` skill.
