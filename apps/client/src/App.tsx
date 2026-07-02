import { useMemo } from "react";
import "./styles.css";
import { AppShell } from "./components/AppShell";
import type { Commands, StateSource } from "./data/StateSource";
import { MockBackend } from "./data/mock";

// The Rust/Tauri bridge does not exist yet (phase 5). Until it does, the whole
// board is driven by an in-memory mock that emits an evolving demo snapshot.
// When the bridge lands, select TauriStateSource/TauriCommands here while
// `__TAURI_INTERNALS__` is present — the interfaces are identical.
export function App() {
  const { source, commands } = useMemo<{ source: StateSource; commands: Commands }>(() => {
    const mock = new MockBackend();
    return { source: mock, commands: mock };
  }, []);

  return <AppShell source={source} commands={commands} />;
}
