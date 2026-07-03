import { useEffect, useRef, useState } from "react";
import { useTheme } from "../theme/ThemeProvider";

export interface ZellijTokenDialogProps {
  token: string;
  onClose: () => void;
}

/**
 * Shown once, right after the bridge creates the first zellij web login
 * token (zellij can never re-display a token). The user pastes it into the
 * login form in the terminal window that just opened.
 */
export function ZellijTokenDialog({ token, onClose }: ZellijTokenDialogProps) {
  const { palette: P } = useTheme();
  const inputRef = useRef<HTMLInputElement>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    inputRef.current?.select();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(token);
      setCopied(true);
    } catch {
      inputRef.current?.select();
    }
  };

  return (
    <div className="ab-modal-scrim" onMouseDown={onClose}>
      <div
        className="ab-modal"
        style={{ backgroundColor: P.mantle, borderColor: P.surface2, color: P.text }}
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <div className="ab-modal-title" style={{ color: P.mauve }}>
          Terminal login token
        </div>
        <div className="ab-modal-body">
          <p style={{ color: P.subtext0, marginBottom: 8 }}>
            Paste this token into the login form in the terminal window. It is shown only once —
            zellij cannot display it again.
          </p>
          <input
            ref={inputRef}
            className="ab-input"
            type="text"
            readOnly
            value={token}
            onFocus={(e) => e.target.select()}
            style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
          />
        </div>
        <div className="ab-modal-actions">
          <button
            type="button"
            className="ab-btn"
            style={{ color: P.crust, backgroundColor: P.green }}
            onClick={copy}
          >
            {copied ? "Copied" : "Copy"}
          </button>
          <button
            type="button"
            className="ab-btn"
            style={{ color: P.text, backgroundColor: P.surface0, borderColor: P.surface2 }}
            onClick={onClose}
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
