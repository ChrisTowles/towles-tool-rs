import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AgentBoardState, AgentDisplay, SessionData } from "../types";
import type { Commands, StateSource } from "../data/StateSource";
import { DEFAULT_THEME } from "../lib/themes";
import { anyAgentRunning, boardCounts } from "../lib/derived";
import { useNow, useSpinner } from "../hooks";
import { ThemeProvider } from "../theme/ThemeProvider";
import { StatusBar } from "./StatusBar";
import { SessionList } from "./SessionList";
import { KillConfirmDialog } from "./KillConfirmDialog";
import { AddRepoDialog } from "./AddRepoDialog";
import { HelpSheet } from "./HelpSheet";
import { Toast } from "./Toast";
import type { ToastData, ToastKind } from "./Toast";
import { ZellijTokenDialog } from "./ZellijTokenDialog";

type Modal =
  | { kind: "kill"; repoName: string }
  | { kind: "addRepo" }
  | { kind: "help" }
  | { kind: "zellijToken"; token: string }
  | null;

export interface AppShellProps {
  source: StateSource;
  commands: Commands;
}

export function AppShell({ source, commands }: AppShellProps) {
  const [state, setState] = useState<AgentBoardState | null>(null);
  const [focusedIdx, setFocusedIdx] = useState(0);
  const [panelFocus, setPanelFocus] = useState<"sessions" | "agents">("sessions");
  const [focusedAgentIdx, setFocusedAgentIdx] = useState(0);
  const [modal, setModal] = useState<Modal>(null);
  const [toast, setToast] = useState<ToastData | null>(null);
  const toastId = useRef(0);

  // Subscribe to the state source for the app's lifetime.
  useEffect(() => {
    const unsub = source.subscribe(setState);
    source.start();
    return () => {
      unsub();
      source.stop();
    };
  }, [source]);

  const sessions = useMemo(() => state?.sessions ?? [], [state]);
  const themeName = state?.theme ?? DEFAULT_THEME;
  const now = useNow(1000);
  const spinIdx = useSpinner(anyAgentRunning(sessions));
  const counts = useMemo(() => boardCounts(sessions), [sessions]);

  // Keep the selection index within range as sessions come and go.
  useEffect(() => {
    if (focusedIdx > sessions.length - 1) {
      setFocusedIdx(Math.max(0, sessions.length - 1));
      setPanelFocus("sessions");
    }
  }, [sessions.length, focusedIdx]);

  const pushToast = useCallback((kind: ToastKind, message: string) => {
    toastId.current += 1;
    setToast({ id: toastId.current, kind, message });
  }, []);

  // Surface command rejections (a failed ab_* invoke) as error toasts instead
  // of silent console noise.
  useEffect(() => commands.onError((msg) => pushToast("error", msg)), [commands, pushToast]);

  const selectIndex = useCallback(
    (i: number) => {
      const s = sessions[i];
      if (!s) return;
      setFocusedIdx(i);
      setPanelFocus("sessions");
      if (s.unseen) commands.markSeen(s.name);
    },
    [sessions, commands],
  );

  const enterAgents = useCallback(
    (i: number, agentIdx: number) => {
      const s = sessions[i];
      if (!s || s.agents.length === 0) return;
      setFocusedIdx(i);
      setPanelFocus("agents");
      setFocusedAgentIdx(Math.min(agentIdx, s.agents.length - 1));
      if (s.unseen) commands.markSeen(s.name);
    },
    [sessions, commands],
  );

  const dismissAgent = useCallback(
    (s: SessionData, a: AgentDisplay) => {
      commands.dismissAgent(s.name, a.agent, a.threadId);
      pushToast("success", `Dismissed ${a.threadName ?? a.agent}`);
    },
    [commands, pushToast],
  );

  const removeRepo = useCallback(
    (name: string) => {
      commands.removeRepo(name);
      setModal(null);
      pushToast("info", `Removed ${name}`);
    },
    [commands, pushToast],
  );

  const addRepo = useCallback(
    (path: string) => {
      commands.addRepo(path);
      setModal(null);
      pushToast("info", `Adding ${path}`);
    },
    [commands, pushToast],
  );

  // Opens the zellij terminal window; a token comes back only on first-ever
  // open (freshly created) and must be shown once for the login form.
  const openTerminal = useCallback(async () => {
    const token = await commands.openZellij();
    if (token) setModal({ kind: "zellijToken", token });
  }, [commands]);

  // Global keymap. Modals attach their own capture-phase handlers, so skip when
  // one is open.
  useEffect(() => {
    if (modal) return;
    const onKey = (e: KeyboardEvent) => {
      const selected = sessions[focusedIdx];
      switch (e.key) {
        case "j":
        case "ArrowDown":
          e.preventDefault();
          if (panelFocus === "agents" && selected) {
            setFocusedAgentIdx((n) => Math.min(selected.agents.length - 1, n + 1));
          } else {
            selectIndex(Math.min(sessions.length - 1, focusedIdx + 1));
          }
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          if (panelFocus === "agents") {
            setFocusedAgentIdx((n) => Math.max(0, n - 1));
          } else {
            selectIndex(Math.max(0, focusedIdx - 1));
          }
          break;
        case "l":
        case "ArrowRight":
          e.preventDefault();
          if (panelFocus === "sessions") enterAgents(focusedIdx, 0);
          break;
        case "h":
        case "ArrowLeft":
        case "Escape":
          e.preventDefault();
          setPanelFocus("sessions");
          break;
        case "d":
          if (panelFocus === "agents" && selected?.agents[focusedAgentIdx]) {
            e.preventDefault();
            dismissAgent(selected, selected.agents[focusedAgentIdx]);
          }
          break;
        case "x":
          if (selected) {
            e.preventDefault();
            setModal({ kind: "kill", repoName: selected.name });
          }
          break;
        case "r":
          e.preventDefault();
          commands.refresh();
          pushToast("info", "Refreshed");
          break;
        case "t":
          e.preventDefault();
          void openTerminal();
          break;
        case "?":
          e.preventDefault();
          setModal({ kind: "help" });
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    modal,
    sessions,
    focusedIdx,
    focusedAgentIdx,
    panelFocus,
    selectIndex,
    enterAgents,
    dismissAgent,
    openTerminal,
    commands,
    pushToast,
  ]);

  return (
    <ThemeProvider themeName={themeName}>
      <div className="ab-app">
        <StatusBar
          counts={counts}
          themeName={themeName}
          onThemeChange={(t) => commands.setTheme(t)}
          onAddRepo={() => setModal({ kind: "addRepo" })}
          onOpenTerminal={() => void openTerminal()}
        />

        <main className="ab-main">
          <SessionList
            sessions={sessions}
            focusedIdx={focusedIdx}
            focusedAgentIdx={focusedAgentIdx}
            panelFocus={panelFocus}
            spinIdx={spinIdx}
            now={now}
            onSelect={selectIndex}
            onDismissAgent={dismissAgent}
            onFocusAgent={(s, _a, idx) => enterAgents(sessions.indexOf(s), idx)}
            onAddRepo={addRepo}
          />
        </main>

        <footer className="ab-footer">
          {panelFocus === "agents"
            ? "[← back] [⏎ focus] [d dismiss] [x remove repo]"
            : "? help"}
        </footer>

        {toast && <Toast toast={toast} onDismiss={() => setToast(null)} />}

        {modal?.kind === "kill" && (
          <KillConfirmDialog
            repoName={modal.repoName}
            onConfirm={() => removeRepo(modal.repoName)}
            onCancel={() => setModal(null)}
          />
        )}
        {modal?.kind === "addRepo" && (
          <AddRepoDialog onSubmit={addRepo} onCancel={() => setModal(null)} />
        )}
        {modal?.kind === "help" && <HelpSheet onClose={() => setModal(null)} />}
        {modal?.kind === "zellijToken" && (
          <ZellijTokenDialog token={modal.token} onClose={() => setModal(null)} />
        )}
      </div>
    </ThemeProvider>
  );
}
