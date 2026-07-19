/**
 * Per-repo chosen icon + color ("repo identity").
 *
 * A repo may carry an optional `meta` blob (see `RepoData` in
 * `lib/agentboard.ts`) picked by the user in Settings → Agentboard and
 * persisted by Rust. Everything here treats that blob as **untrusted**: an
 * unknown icon name or a malformed color must degrade to the default repo
 * look (`FolderGit2`, `text-muted-foreground`), never crash a render and
 * never invent a fallback color.
 *
 * Colors are free-form hex, so they cannot be Tailwind classes. The one
 * blessed seam for turning a hex into pixels is `repoAccentStyles` — call
 * sites apply the returned inline styles and never hand-roll color math.
 */
import {
  Anchor,
  BookOpen,
  Bot,
  Boxes,
  Brain,
  Bug,
  Cloud,
  Code,
  Cog,
  Compass,
  Container,
  Cpu,
  Database,
  FlaskConical,
  FolderGit2,
  Gauge,
  Globe,
  Hammer,
  Layers,
  Leaf,
  Package,
  Palette,
  Plane,
  Puzzle,
  Radio,
  Rocket,
  Server,
  Shield,
  Sparkles,
  Terminal,
  Wrench,
  Zap,
  type LucideIcon,
} from "lucide-react";
import type { CSSProperties } from "react";

/** How a repo's color is expressed on a surface: a thin accent edge + tinted
 * glyph (`accent`, the default) or that plus a soft background wash
 * (`tint`). Absent ⇒ `"accent"`. */
export type RepoIdentityStyle = "accent" | "tint";

/** The optional per-repo identity blob, exactly as Rust stores/returns it.
 * Every field is optional and every value is untrusted — validate on read. */
export type RepoMeta = {
  icon?: string;
  color?: string;
  style?: RepoIdentityStyle;
};

/** The curated icon allowlist — keys are lucide component names, which is what
 * gets persisted. Chosen to stay readable at 14px and to say something useful
 * about a repo (what it *is*, not how it feels). Adding a name here is the
 * only way to make it selectable; the store is never trusted to name an icon
 * outside this map. */
export const REPO_ICONS: Record<string, LucideIcon> = {
  FolderGit2,
  Rocket,
  Bug,
  Boxes,
  Terminal,
  Cloud,
  Database,
  Cpu,
  Globe,
  BookOpen,
  Wrench,
  FlaskConical,
  Palette,
  Zap,
  Shield,
  Package,
  Server,
  Code,
  Layers,
  Sparkles,
  Hammer,
  Radio,
  Compass,
  Bot,
  Plane,
  Cog,
  Anchor,
  Brain,
  Container,
  Gauge,
  Leaf,
  Puzzle,
};

/** The icon a repo with no chosen identity renders — also the fallback for an
 * unknown name. */
export const DEFAULT_REPO_ICON: LucideIcon = FolderGit2;

/** Default swatches offered in the color picker. Mid-chroma so they stay
 * legible on both the light and the dark app surface (nothing near-white or
 * near-black), and deliberately clear of the rail's reserved status hues —
 * amber (needs-you), violet (agent/focus), and sky-500, which marks the
 * primary checkout's branch label — so a repo's decoration can never be
 * mistaken for a signal. Free-form hex can still pick any of them; this is
 * only the fast path. Free-form hex is still allowed; this is
 * just the fast path. */
export const REPO_PALETTE: readonly string[] = [
  "#e11d48",
  "#ec4899",
  "#d946ef",
  "#3b82f6",
  "#0891b2",
  "#14b8a6",
  "#059669",
  "#65a30d",
  "#78716c",
];

/** Resolve a repo's icon from the allowlist. An absent or unrecognized name
 * falls back to `FolderGit2` — the store is untrusted input. */
export function repoIcon(meta: RepoMeta | null | undefined): LucideIcon {
  const name = meta?.icon;
  if (!name) return DEFAULT_REPO_ICON;
  return REPO_ICONS[name] ?? DEFAULT_REPO_ICON;
}

const HEX_RE = /^#?(?:[0-9a-f]{3}|[0-9a-f]{6})$/i;

/** Mirrors the Rust color parser: `#rgb` or `#rrggbb`, the `#` optional,
 * case-insensitive. */
export function isHexColor(s: string): boolean {
  return HEX_RE.test(s.trim());
}

/** Canonicalize a user-typed color to lowercase `#rrggbb`, or `null` when it
 * isn't a color at all. Shorthand `#abc` expands to `#aabbcc`. */
export function normalizeHex(s: string): string | null {
  const raw = s.trim();
  if (!isHexColor(raw)) return null;
  const body = (raw.startsWith("#") ? raw.slice(1) : raw).toLowerCase();
  if (body.length === 3) {
    return `#${body[0]}${body[0]}${body[1]}${body[1]}${body[2]}${body[2]}`;
  }
  return `#${body}`;
}

/** The inline styles a repo's identity contributes to one surface.
 *
 * Every field is `undefined` when the repo has no (valid) color, so a call
 * site can spread/pass them unconditionally and get today's rendering for an
 * unthemed repo.
 *
 * - `iconStyle` — tints the glyph. Apply *instead of* `text-muted-foreground`.
 * - `edgeStyle` — colors a `border-l-2` edge. Callers must only apply it when
 *   no status accent (amber needs-you, violet active) owns the edge; identity
 *   never outranks attention.
 * - `surfaceStyle` — the soft background wash, present only for `style: "tint"`.
 *
 * `color-mix(in srgb, …, transparent)` is used rather than a fixed rgba so the
 * wash and edge stay proportionate against both the light and dark surface.
 */
export type RepoAccentStyles = {
  iconStyle: CSSProperties | undefined;
  edgeStyle: CSSProperties | undefined;
  surfaceStyle: CSSProperties | undefined;
};

const EMPTY_ACCENT: RepoAccentStyles = {
  iconStyle: undefined,
  edgeStyle: undefined,
  surfaceStyle: undefined,
};

export function repoAccentStyles(
  meta: RepoMeta | null | undefined,
  /** What the tint wash mixes into. Defaults to `transparent`, which lets
   * whatever is behind the element show through. A **sticky** surface (the
   * rail's repo header, which rows scroll underneath) must stay fully
   * opaque, so it passes its own background token instead — e.g.
   * `"var(--card)"`. */
  base = "transparent",
): RepoAccentStyles {
  const hex = meta?.color ? normalizeHex(meta.color) : null;
  if (!hex) return EMPTY_ACCENT;
  const style: RepoIdentityStyle = meta?.style ?? "accent";
  return {
    iconStyle: { color: hex },
    edgeStyle: { borderLeftColor: `color-mix(in srgb, ${hex} 70%, transparent)` },
    surfaceStyle:
      style === "tint" ? { backgroundColor: `color-mix(in srgb, ${hex} 8%, ${base})` } : undefined,
  };
}

/** True when the repo has a usable identity color — the cheap test a call
 * site uses to decide whether to drop `text-muted-foreground`. */
export function hasRepoColor(meta: RepoMeta | null | undefined): boolean {
  return meta?.color ? normalizeHex(meta.color) !== null : false;
}
