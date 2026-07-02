import { useEffect } from "react";
import { useTheme } from "../theme/ThemeProvider";

export interface KillConfirmDialogProps {
  repoName: string;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * Confirm removing a repo from the board (UI-SPEC §3 — the TUI's `x` kill
 * modal, relabelled "remove repo" for the desktop app). y/Enter confirms,
 * n/Esc cancels.
 */
export function KillConfirmDialog({ repoName, onConfirm, onCancel }: KillConfirmDialogProps) {
  const { palette: P } = useTheme();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "y" || e.key === "Enter") {
        e.preventDefault();
        onConfirm();
      } else if (e.key === "n" || e.key === "Escape") {
        e.preventDefault();
        onCancel();
      }
      e.stopPropagation();
    };
    // Capture so the global board keymap does not also handle these keys.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onConfirm, onCancel]);

  return (
    <div className="ab-modal-scrim" onMouseDown={onCancel}>
      <div
        className="ab-modal"
        style={{ backgroundColor: P.mantle, borderColor: P.surface2, color: P.text }}
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <div className="ab-modal-title" style={{ color: P.peach }}>
          Remove repo
        </div>
        <div className="ab-modal-body">
          Remove <strong style={{ color: P.text }}>{repoName}</strong> from the board?
        </div>
        <div className="ab-modal-actions">
          <button
            type="button"
            className="ab-btn ab-btn-danger"
            style={{ color: P.crust, backgroundColor: P.red }}
            onClick={onConfirm}
          >
            Remove (y)
          </button>
          <button
            type="button"
            className="ab-btn"
            style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
            onClick={onCancel}
          >
            Cancel (n)
          </button>
        </div>
      </div>
    </div>
  );
}
