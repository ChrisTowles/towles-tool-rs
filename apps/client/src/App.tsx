import { useMemo } from "react";
import "./styles.css";
import { AppShell } from "./components/AppShell";
import type { Commands, StateSource } from "./data/StateSource";
import { MockBackend } from "./data/mock";
import { TauriCommands, TauriStateSource } from "./data/TauriStateSource";

// Tauri injects `__TAURI_INTERNALS__` on window inside the desktop shell. When
// present, drive the board from the real Rust bridge; otherwise use the mock so
// `npm run client:dev` in a bare browser stays a living demo. The interfaces
// are identical, so nothing downstream changes.
const isTauri = "__TAURI_INTERNALS__" in window;

export function App() {
  const { source, commands } = useMemo<{ source: StateSource; commands: Commands }>(() => {
    if (isTauri) {
      return { source: new TauriStateSource(), commands: new TauriCommands() };
    }
    const mock = new MockBackend();
    return { source: mock, commands: mock };
  }, []);

  return <AppShell source={source} commands={commands} />;
}
