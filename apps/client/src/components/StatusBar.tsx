import { useTheme } from "../theme/ThemeProvider";
import { THEME_NAMES } from "../lib/themes";
import type { BoardCounts } from "../lib/derived";

export interface StatusBarProps {
  counts: BoardCounts;
  themeName: string;
  onThemeChange: (theme: string) => void;
  onAddRepo: () => void;
}

/** Header + board counts (UI-SPEC §2) plus the add-repo + theme controls. */
export function StatusBar({ counts, themeName, onThemeChange, onAddRepo }: StatusBarProps) {
  const { palette: P } = useTheme();
  return (
    <header className="ab-statusbar">
      <div className="ab-statusbar-text">
        <div className="ab-title" style={{ color: P.mauve }}>
          AgentBoard
        </div>
        <div className="ab-counts">
          <span style={{ color: P.overlay0 }}>{counts.sessionCount}s</span>
          {counts.runningCount > 0 && (
            <span style={{ color: P.yellow }}> ⚡{counts.runningCount}</span>
          )}
          {counts.errorCount > 0 && <span style={{ color: P.red }}> ✗{counts.errorCount}</span>}
          {counts.unseenCount > 0 && <span style={{ color: P.teal }}> ●{counts.unseenCount}</span>}
        </div>
      </div>
      <div className="ab-statusbar-controls">
        <button
          type="button"
          className="ab-add-btn"
          title="Add repo"
          aria-label="Add repo"
          onClick={onAddRepo}
          style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
        >
          +
        </button>
        <label className="ab-theme-picker" style={{ color: P.overlay0 }}>
          theme
          <select
            value={themeName}
            onChange={(e) => onThemeChange(e.target.value)}
            style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
          >
            {THEME_NAMES.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </label>
      </div>
    </header>
  );
}
