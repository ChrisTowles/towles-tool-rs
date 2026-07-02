import { useEffect } from "react";
import { useTheme } from "../theme/ThemeProvider";

export type ToastKind = "info" | "success" | "error";

export interface ToastData {
  id: number;
  kind: ToastKind;
  message: string;
}

/** Transient command feedback; auto-dismisses after 4s (UI-SPEC §2). */
export function Toast({ toast, onDismiss }: { toast: ToastData; onDismiss: () => void }) {
  const { palette: P } = useTheme();
  useEffect(() => {
    const id = setTimeout(onDismiss, 4000);
    return () => clearTimeout(id);
  }, [toast.id, onDismiss]);

  const color = toast.kind === "error" ? P.red : toast.kind === "success" ? P.green : P.blue;
  return (
    <div
      className="ab-toast"
      style={{ backgroundColor: P.mantle, borderColor: color, color: P.text }}
      role="status"
    >
      <span style={{ color }}>●</span> {toast.message}
    </div>
  );
}
