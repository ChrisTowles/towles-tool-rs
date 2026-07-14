/**
 * Dictation bridge: Tauri `dictation_*` commands + `dictation://*` events
 * (see `crates-tauri/tt-app/src/dictation.rs`) wired to a target element.
 *
 * Scope: this wires the "focused webview input" target from issue #207 —
 * `useDictationForElement` below. The terminal and standalone-panel targets
 * described in that issue aren't wired yet; `dictation-retype.ts`'s
 * `committedDelta` is ready for the terminal target when that lands.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invokeCmd, invokeOk, isTauri } from "./tauri";
import { RetypeState } from "./dictation-retype";

export type DictationPhase = "idle" | "loadingModel" | "recording" | "stopping";

export interface DictationStatePayload {
  phase: DictationPhase;
  sessionId: string | null;
  error: string | null;
}

export interface DictationTranscriptPayload {
  sessionId: string;
  seq: number;
  committed: string[];
  liveTail: string;
  text: string;
}

export interface DictationLevelPayload {
  dbfs: number;
}

export const dictationStatus = () =>
  invokeCmd<{ phase: DictationPhase; sessionId: string | null }>("dictation_status");
export const dictationStart = () => invokeOk("dictation_start");
export const dictationStop = () => invokeOk("dictation_stop");
export const dictationToggle = () => invokeOk("dictation_toggle");
export const dictationModelStatus = () => invokeCmd<boolean>("dictation_model_status");
export const dictationModelFetch = () => invokeOk("dictation_model_fetch");
export const dictationDevices = () => invokeCmd<string[]>("dictation_devices");

/**
 * Drives dictation into a single React-controlled `<input>`/`<textarea>`:
 * `start()` kicks off (or reuses) a recording session and retypes every
 * transcript update into `elRef.current`; `stop()` ends it. Only one session
 * is tracked per hook instance — starting a second while one is active is a
 * no-op (mirrors the backend's own idempotent `dictation_start`).
 *
 * Unmount safety: the effect below stops listening on unmount, but does NOT
 * stop the backend session — a dialog closing mid-dictation shouldn't kill
 * the mic out from under a still-open target elsewhere. Callers that want
 * "close the dialog == stop dictating" should call `stop()` explicitly (e.g.
 * from the dialog's close handler).
 */
export function useDictationForElement(elRef: React.RefObject<HTMLInputElement | HTMLTextAreaElement | null>) {
  const [phase, setPhase] = useState<DictationPhase>("idle");
  const [error, setError] = useState<string | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const retypeRef = useRef<RetypeState | null>(null);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlistenState: (() => void) | undefined;
    let unlistenTranscript: (() => void) | undefined;

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");

      const stateSub = await listen<DictationStatePayload>("dictation://state", (e) => {
        const payload = e.payload;
        setPhase(payload.phase);
        setError(payload.error);
        if (payload.phase === "recording" && payload.sessionId) {
          sessionIdRef.current = payload.sessionId;
          retypeRef.current = new RetypeState();
        } else if (payload.phase === "idle") {
          sessionIdRef.current = null;
          retypeRef.current = null;
        }
      });
      if (disposed) {
        stateSub();
        return;
      }
      unlistenState = stateSub;

      const transcriptSub = await listen<DictationTranscriptPayload>("dictation://transcript", (e) => {
        const payload = e.payload;
        if (payload.sessionId !== sessionIdRef.current) return;
        const el = elRef.current;
        const retype = retypeRef.current;
        if (!el || !retype) return;
        retype.applyToElement(el, payload.text);
      });
      if (disposed) {
        transcriptSub();
        return;
      }
      unlistenTranscript = transcriptSub;
    })();

    return () => {
      disposed = true;
      unlistenState?.();
      unlistenTranscript?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- elRef is a ref, stable by contract
  }, []);

  const start = useCallback(() => {
    void dictationStart();
  }, []);

  const stop = useCallback(() => {
    void dictationStop();
  }, []);

  return { phase, error, start, stop, recording: phase === "recording" };
}
