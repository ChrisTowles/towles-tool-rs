import type { SessionData } from "../types";
import { useTheme } from "../theme/ThemeProvider";

/**
 * Git diff summary (UI-SPEC §1B). Each span renders only when its stat is
 * non-zero: `{files}f` overlay0, `+{added}` green, `-{removed}` red,
 * `{delta}↑` sky (>0), `{|delta|}↓` peach (<0).
 */
export function DiffStats({ session }: { session: SessionData }) {
  const { palette: P } = useTheme();
  const { filesChanged, linesAdded, linesRemoved, commitsDelta } = session;
  return (
    <span className="ab-diffstats">
      {!!filesChanged && <span style={{ color: P.overlay0 }}>{filesChanged}f </span>}
      {!!linesAdded && <span style={{ color: P.green }}>+{linesAdded} </span>}
      {!!linesRemoved && <span style={{ color: P.red }}>-{linesRemoved} </span>}
      {commitsDelta > 0 && <span style={{ color: P.sky }}>{commitsDelta}↑</span>}
      {commitsDelta < 0 && <span style={{ color: P.peach }}>{Math.abs(commitsDelta)}↓</span>}
    </span>
  );
}
