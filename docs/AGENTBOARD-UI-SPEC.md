# Porting spec — TUI rendering & interaction (agentboard phase 3 UI)

Source: slot-1 `packages/agentboard/src/tui/` — `index.tsx` (797),
`components/SessionCard.tsx` (397), `StatusBar.tsx` (49), `DiffStats.tsx`,
formatters (`elapsed`, `short-model`, `status-visuals`, `family-color`),
`constants.ts`, `runtime/themes.ts`. Spec taken 2026-07-02. OpenTUI layout
mechanics ignored — this is the visual/behavioral contract for the React
rebuild. **⚠ = flag; ◧ = tmux-specific.**

## 1. SessionCard anatomy (top→bottom)

Card = left accent bar + main column. Card bg = `surface0` when focused, else
transparent. Whole card click → select.

**A. Accent bar** (leftmost col):
- `▌` in `accentColor()`, OR a space when `accentColor === "transparent"`.
- When transparent, a second col shows a dim `▎` in the session's
  `familyColor` (always-present left edge tint).
- `accentColor()` precedence: `isCurrent→green` ·
  `unseenTerminal→unseenTerminalColor(status)` · `error→red` ·
  `interrupted→peach` · `running→yellow` · `waiting→blue` ·
  `question→green` · `isFocused→lavender` · else transparent.

**B. Header row**:
1. **Name** — `truncate(name, 18)`; color `isFocused→text` ·
   `isCurrent→subtext1` · else `familyColor`; bold if focused or current.
2. **DiffStats** (only if any stat truthy) — spans rendered only if non-zero:
   `{filesChanged}f ` (overlay0), `+{linesAdded} ` (green),
   `-{linesRemoved} ` (red), `{commitsDelta}↑` (sky, >0),
   `{abs(commitsDelta)}↓` (peach, <0).
3. **Status cell** (width 3) — if `statusIcon()`:
   ` {icon}{runningCount if >1}` in `statusColor()`.
   `statusIcon` = `liveStatusIcon(status, spinIdx)` (running→spinner frame,
   waiting→`◉`, question→`?`, else "") else `●` if unseen-terminal, else "".
   `statusColor` = unseen-terminal→`unseenTerminalColor`, else
   `theme.status[status]`.

**C. Branch row** (only if `branch`) — `truncate(branch, 45)`;
`isFocused→pink` else `overlay0`.

**D. Metadata summary row** (only if non-empty) — dim,
color = `toneColor(metadata.status.tone)`. Text = join(` · `) of
`[status.text, progress (cur/total OR "{round(percent*100)}%"),
progress.label]`.

**E. Agent rows** — one AgentRow per `session.agents[]`.

### AgentRow

Bg: flash→`surface2`, keyboard-focused→`surface1`, else transparent. Row
click: 150ms flash + focus action (dismiss ✕ excluded via stopPropagation).

- **Line 1**: icon + threadName + elapsed + dismiss.
  - **icon**: unseen→`●` · done→`✓` · error→`✗` · interrupted→`⚠` · else
    `liveStatusIcon || ○`. Color: terminal+unseen→`unseenTerminalColor` ·
    error→red · interrupted→peach · terminal→green · else
    `theme.status[status]`.
  - **threadName**: `truncate(collapseWS(threadName), 40)`; unseen→icon
    color else overlay0.
  - **elapsed** (only `running` && `details.lastActivityAt`):
    `formatElapsed(now - lastActivityAt)`; dim.
  - **dismiss ✕**: always shown; overlay0, red on hover; own handler.
- **Line 2 — model/tool** (only `running` && details && (model||tool)):
  `shortModel(model)` (subtext0 dim), if tool: ` · ` (overlay0) + `⟶ `
  (teal) + `{lastTool}` (subtext0).
- **Subagents block** (only `running` && `details.subagents.length`): `⚡ `
  (mauve) + `{n} agent(s)` (subtext0); per subagent: `  ↳ ` (overlay0) +
  `{agentType}` (teal) + ` · ` + `truncate(collapseWS(description), 40)`
  (subtext0).
- **Loop line** (only `details.loop && nextWakeAt > now`): `⟳ ` (lavender) +
  `loops in {formatElapsed(nextWakeAt - now)}` (subtext0) + optional
  ` · {truncate(collapseWS(reason), 36)}` (overlay0).
- **Cache line** (only if `cacheLabel()`): overlay0 dim.
  `expiresAt = cacheExpiresAt ?? (lastActivityAt + 1h)`;
  `minutesLeft = ceil((expiresAt - now)/60000)`; `"cache expired"` if ≤0
  else `"cache {minutesLeft}m"`.

**Formatters (exact):**
- `formatElapsed(ms)`: clamp ≥0; `<60s→"{s}s"`, `<60m→"{m}m"`, else `"{h}h"`.
- `shortModel(model)`: strip leading `claude-` and trailing `[1m]`
  (`claude-opus-4-6` → `opus-4-6`).
- `truncate(str, n)`: hard char cap with ellipsis.
- Spinner frames `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` (120ms, ticking only while any agent runs).

## 2. StatusBar + footer

- Line 1: `  AgentBoard` in mauve bold.
- Line 2: `{sessionCount}s` (overlay0) + `⚡{runningCount}` (yellow, >0) +
  `✗{errorCount}` (red, >0) + `●{unseenCount}` (teal, >0). running/error =
  sum over sessions' agents; unseen = count of sessions flagged unseen.
- Footer: divider; toast if active (4s auto-dismiss; error→red,
  success→green, info→blue); hint line (`? help`, or in agents panel:
  `[← back] [⏎ focus] [d dismiss] [x kill]`).

## 3. Interaction model — carry vs drop

| behavior | TS | desktop |
|---|---|---|
| focusedSession selection | local per-TUI, never sent to server | **Port** (local UI state) |
| panelFocus sessions↔agents | →/l in, ←/h/Esc out | **Port** (master/detail or expandable card) |
| currentSession / pendingSwitch ◧ | tmux "you-are-here" + optimistic switch | **Drop** (optionally "last-opened repo") |
| kill-confirm modal | `x` → y/n overlay | **Port** as confirm dialog; action = remove repo |
| help overlay | `?` keybinding sheet | **Port** (lower priority) |
| toasts | transient command feedback | **Port** |
| dismiss-agent (✕/`d`) | remove instance | **Port** |
| mark-seen | implicit on focus | **Port** — clear unseen on card view/expand |
| open-in-editor (`e`) | `$preferredEditor <dir>` (TMUX env stripped) | **Port** (drop env stripping) |
| reorder (Alt+↑↓ etc.) | reorder-session, persists | **Port** (drag or keyboard) |
| switch-session/index, tab-cycle, 1-9 ◧ | tmux client switching | **Drop** |
| new-session (`n`) ◧ | tmux sessionizer fzf | **Replace** with "add repo" dialog |
| focus/kill-agent-pane ◧ | tmux pane targeting | **Drop** (agent-row click could open dir/journal) |
| identify-pane / re-identify ◧ | tmux bookkeeping | **Drop** |

Full TS keymap for the shortcuts sheet: `q` quit · `Esc/←/h` back · `↑k ↓j`
move · `→/l` agents panel · `Enter` activate · `Tab` cycle · `r` refresh ·
`d` dismiss · `x` kill · `e` editor · `n` new · `1-9` jump · `Alt+↑↓`
reorder · `Alt+Shift+↑↓` top/bottom · `?` help.

## 4. Theme system

`Theme = { palette, status: Record<AgentStatus,string>, icons }`.

- **palette** — 21 Catppuccin-style keys used by all themes:
  `blue lavender pink mauve yellow green red peach teal sky` (accents) ·
  `text subtext0 subtext1` (fg tiers) · `overlay0 overlay1` (dim) ·
  `surface0 surface1 surface2` (raised bgs) · `base mantle crust` (bg tiers).
- **status** — per-AgentStatus color.
- **icons** ⚠ — near-vestigial: components use `liveStatusIcon` + hardcoded
  glyphs (`✓ ✗ ⚠ ● ◉ ? ○` + spinner), not `theme.icons`.
- **18 built-ins**: catppuccin-mocha/latte/frappe/macchiato, tokyo-night,
  gruvbox-dark, nord, dracula, github-dark, one-dark, kanagawa, everforest,
  material, flexoki, ayu, aura, matrix, transparent.
  **Default `catppuccin-mocha`.**
- **resolveTheme(string | PartialTheme | undefined)**: undefined→default;
  string→builtin ?? default; partial→shallow-merge sections over default.
- **Switching**: `set-theme` → server persists `agentboard.theme` and
  rebroadcasts; every state message carries `theme`. Theme is
  **server-owned/global**, not per-client.

**UI-role → palette map**: app bg `crust` · focused card `surface0` ·
kbd-focused agent row `surface1` · flash `surface2` · modal `mantle` ·
header/subagent-icon `mauve` · focused name `text` / branch `pink` ·
secondary `subtext0/1` · dim `overlay0/1`. Status accents: running `yellow`,
waiting `blue`, done/question `green`, error `red`, interrupted `peach`,
unseen-terminal `teal`, focused `lavender`. `toneColor`: success→green,
error→red, warn→yellow, info→blue, default→overlay0.

**family-color** ⚠: groups sessions by "family" — `KNOWN_FAMILIES` is
hardcoded to personal repos (`blog→pink, dotfiles→peach, f→teal,
toolbox→sky, towles-tool→lavender`); `familyOf` strips
`-primary`/`-slot-N` suffixes; unknowns hash into
`[mauve, blue, green, yellow, red]`. Port decision: **keep the hash
fallback, drop the hardcoded map** (or make it configurable later).

## 5. Empty / edge states

- No sessions: empty list, `0s` in StatusBar, no message in TS ⚠ — desktop
  adds a "No repos configured" empty state with an add-repo affordance.
- Session without agents: minimal name+branch(+diff) card, no icon,
  transparent accent.
- Metadata-only card: name row + metadata summary line.
- Idle agent: `○`, no elapsed/model/cache lines (all gate on `running`).
- Multiple running agents: count appended after the status icon.
- Truncation caps: name 18, branch 45, threadName 40, subagent desc 40,
  loop reason 36 (hard cuts; whitespace collapsed on
  threadName/desc/reason).

## 6. React port plan (adopted)

**Components**: `AppShell` (subscribes to the Tauri state event; holds local
selection/panelFocus/modal/toast; dispatches commands) → `StatusBar`,
`SessionList` → `SessionCard` (`AccentBar`, `CardHeader` = name +
`DiffStats` + `StatusCell`, `BranchRow`, `MetaSummaryRow`) → `AgentRow`
(`AgentHeader`, `ModelToolLine`, `SubagentsBlock`, `LoopLine`, `CacheLine`)
→ overlays `KillConfirmDialog` (remove repo), `HelpSheet`, `Toast`;
`ThemeProvider` mapping palette keys to CSS custom properties (theme switch
= one var swap).

**Pure functions ported verbatim as TS**: `formatElapsed`, `shortModel`,
`liveStatusIcon` + `unseenTerminalColor`, `familyColor`/`familyOf`/hash
(hash fallback only), `toneColor`, `truncate`, `resolveTheme` +
`BUILTIN_THEMES` data. The derived helpers (`accentColor`, `statusColor`,
`statusIcon`, agent `icon`/`color`, `cacheLabel`, metadata summary) are pure
over `(session/agent, theme, now, flags)` — extract as testable functions,
not inline JSX. Icons: adopt the fixed effective glyph set; skip the
vestigial `theme.icons` indirection.
