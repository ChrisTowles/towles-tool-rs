import { useEffect, useRef, useState } from "react";
import { useTheme } from "../theme/ThemeProvider";

export interface AddRepoDialogProps {
  onSubmit: (path: string) => void;
  onCancel: () => void;
}

/**
 * Modal for adding a repo by absolute path (wired to `ab_add_repo`). Path
 * validity is enforced by the bridge — an invalid path rejects and surfaces as
 * an error toast, so this only guards against empty input.
 */
export function AddRepoDialog({ onSubmit, onCancel }: AddRepoDialogProps) {
  const { palette: P } = useTheme();
  const [path, setPath] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onCancel]);

  const submit = () => {
    const trimmed = path.trim();
    if (trimmed) onSubmit(trimmed);
  };

  return (
    <div className="ab-modal-scrim" onMouseDown={onCancel}>
      <div
        className="ab-modal"
        style={{ backgroundColor: P.mantle, borderColor: P.surface2, color: P.text }}
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <div className="ab-modal-title" style={{ color: P.mauve }}>
          Add repo
        </div>
        <div className="ab-modal-body">
          <input
            ref={inputRef}
            className="ab-input"
            type="text"
            placeholder="/absolute/path/to/repo"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                submit();
              }
            }}
            style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
          />
        </div>
        <div className="ab-modal-actions">
          <button
            type="button"
            className="ab-btn"
            style={{ color: P.crust, backgroundColor: P.green }}
            onClick={submit}
            disabled={!path.trim()}
          >
            Add
          </button>
          <button
            type="button"
            className="ab-btn"
            style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
            onClick={onCancel}
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
