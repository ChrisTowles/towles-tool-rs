import { useEffect } from "react";
import { useTheme } from "../theme/ThemeProvider";

/** Ported subset of the TS keymap (UI-SPEC §3), tmux-only bindings dropped. */
const KEYS: [string, string][] = [
  ["↑ k / ↓ j", "move selection"],
  ["→ / l", "into agents panel"],
  ["← / h / Esc", "back to list"],
  ["d", "dismiss focused agent"],
  ["x", "remove repo (confirm)"],
  ["r", "refresh"],
  ["t", "open terminal (zellij)"],
  ["?", "toggle this help"],
  ["click card", "select · mark seen"],
  ["click ✕", "dismiss agent"],
];

export function HelpSheet({ onClose }: { onClose: () => void }) {
  const { palette: P } = useTheme();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "?" || e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="ab-modal-scrim" onMouseDown={onClose}>
      <div
        className="ab-modal ab-help"
        style={{ backgroundColor: P.mantle, borderColor: P.surface2, color: P.text }}
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <div className="ab-modal-title" style={{ color: P.mauve }}>
          Keyboard shortcuts
        </div>
        <dl className="ab-help-list">
          {KEYS.map(([key, desc]) => (
            <div className="ab-help-row" key={key}>
              <dt style={{ color: P.text }}>{key}</dt>
              <dd style={{ color: P.subtext0 }}>{desc}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}
