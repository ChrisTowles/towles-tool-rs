---
name: folder-rail-ui
description: Visual design language ("Folder Rail" style) for new or restyled UI in the Towles Tool desktop app (apps/client) — color tokens, agent-status semantics, folder/session hierarchy, spacing, glyphs, Tailwind recipes. Use when adding a new screen/component, restyling an existing one, or the user asks about the app's look, the "folder rail", status dots/colors, or the repo→folder→session hierarchy. Not needed for logic-only changes to already-styled components.
user-invocable: true
---

# Folder Rail — visual language cheat sheet

Neutral grayscale shadcn base (`apps/client/src/index.css`); a hue is added
only to carry agent status or attention, never decoration.

**Hierarchy:** repo (1..N folders: clone/worktree/slot) → folder (1..N
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
| `busy` | `bg-yellow-500` (`animate-pulse` while live only) |
| `waiting` | `bg-blue-500` |
| `error` | `bg-red-500` |
| `complete` | `bg-green-500` |
| `interrupted` | `bg-orange-500` |
| `idle` (default) | `bg-muted-foreground/40` |

Dot = `size-2 rounded-full`.

## Two accent hues, one rule each
- **Amber (`amber-500`)** = needs-you (`status ∈ {waiting, error}` + failing
  PRs). Left border `border-l-2 border-l-amber-500`, folder count badge
  `text-amber-500 border border-amber-500/50 bg-amber-500/10`.
- **Violet (`violet-500`)** = agent-ness / currently focused. Agent glyph `✦`,
  active row `border-l-2 border-l-violet-500 bg-accent`, `+ session` action,
  terminal prompt caret.
- If a row is both active and needs-you: **amber wins the border**; show
  violet elsewhere (glyph, tab).

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

// session row
<button className={cn(
  "flex items-center gap-2.5 py-1.5 pr-3 pl-9 text-left border-l-2 border-transparent",
  hovered && "bg-accent",
  active && "bg-accent border-l-violet-500",
  needsYou && "border-l-amber-500",
)}>
  <span className={cn("font-mono text-xs w-4 text-center",
    type === "agent" ? "text-violet-500" : "text-muted-foreground/60")}>
    {type === "agent" ? "✦" : "❯"}
  </span>
  <span className={cn("size-2 rounded-full", statusColor(status))} />
  <span className="text-foreground">{name}</span>
  <span className="ml-auto text-[11px] text-muted-foreground">{message}</span>
  {needsYou && <span className="size-1.5 rounded-full bg-amber-500" />}
</button>

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
- Don't animate resting UI — `animate-pulse` is for a live/busy dot only.
- Don't let violet (focus) override amber (attention) on a row's border.

## Source of truth in the app
`apps/client/src/lib/agentboard.ts` (`statusColor()`, types) ·
`apps/client/src/screens/agentboard.tsx` (`KIND_BORDER`, rail/split layout) ·
`apps/client/src/components/day-bar.tsx` (needs-you math) ·
`apps/client/src/index.css` (token definitions).
