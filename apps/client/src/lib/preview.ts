import { invoke } from "@/lib/tauri";
import { claudeCommand, type FolderData, isAgent, promptWithImages } from "@/lib/agentboard";
import { devServerUrl, type LaunchConfigStatus } from "@/lib/launch";

/** A dev server detected from some tracked checkout's `.claude/launch.json`
 * (the same file Claude Desktop's preview pane reads) — the view model the
 * Preview screen renders and navigates to. */
export type DevServer = {
  key: string;
  label: string;
  url: string;
  listening: boolean;
  folderDir: string;
};

/** Map a folder's launch.json configs to preview-able dev servers, dropping
 * the port-less ones (they can't be probed or previewed). Pure so the
 * screen's discovery loop stays a thin `Promise.all` over IPC. */
export function devServersOf(
  repoName: string,
  folderDir: string,
  configs: LaunchConfigStatus[],
): DevServer[] {
  return configs
    .filter((cfg) => cfg.port != null)
    .map((cfg) => ({
      key: `${folderDir}\0${cfg.name}`,
      label: `${repoName} · ${cfg.name} :${cfg.port}`,
      url: devServerUrl(cfg.port as number),
      listening: cfg.portListening,
      folderDir,
    }));
}

/** The preview surface's viewport rect in CSS pixels, plus the DPR that maps
 * it into the snapshot's device-pixel space — mirrors `CaptureRect` in
 * `crates-tauri/tt-app/src/preview.rs`. */
export type CaptureRect = {
  x: number;
  y: number;
  width: number;
  height: number;
  devicePixelRatio: number;
};

/** Rasterize the app webview and crop to `rect`, returning a base64 PNG.
 * Backend capture exists because the DOM can't screenshot a cross-origin
 * iframe — see the module docs in `preview.rs`. */
export const previewCapture = (rect: CaptureRect) =>
  invoke<string>("preview_capture", { rect }, { timeoutMs: 15_000 });

/** The wire shape the `preview_write_feedback` command decodes (Rust
 * `PastedImage`) — the persisted subset of agentboard's `PastedImage`, without
 * its UI-only preview fields. */
export type FeedbackImage = { mime: string; dataBase64: string };

/** Stage the annotated capture as files under the pasted-images dir (outside
 * any repo), returning absolute paths for `feedbackPrompt`. */
export const previewWriteFeedback = (repo: string, images: FeedbackImage[]) =>
  invoke<string[]>("preview_write_feedback", { repo, images });

// --- Annotation model ---

export type Point = { x: number; y: number };

export type AnnotationTool = "pen" | "line" | "rect" | "ellipse" | "text";

/** One drawn mark, in CSS-pixel coordinates of the preview surface. `pen`
 * holds the full pointer trail; `line`/`rect`/`ellipse` hold `[from, to]`;
 * `text` holds `[anchor]` plus `text`. One shape instead of a tagged union
 * per tool: every consumer (draw, hit nothing, serialize nothing) treats
 * them uniformly, so the union would only add casts. */
export type Annotation = {
  tool: AnnotationTool;
  color: string;
  points: Point[];
  text?: string;
};

/** Annotation ink. Raw hex (not Tailwind classes) because these are canvas
 * strokes and must survive into the captured PNG identically in both themes;
 * red first so the default matches the "point at the broken thing" use. */
export const ANNOTATION_COLORS = ["#ef4444", "#22c55e", "#3b82f6", "#eab308"] as const;

export const ANNOTATION_STROKE_WIDTH = 3;
export const ANNOTATION_FONT = "600 14px system-ui, sans-serif";

/** Normalize two drag corners into a positive-size rect. */
export function normRect(a: Point, b: Point): { x: number; y: number; w: number; h: number } {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  };
}

/** Paint one annotation. `scale` maps CSS-pixel coordinates to the canvas
 * backing store (the devicePixelRatio the canvas was sized with). */
export function drawAnnotation(ctx: CanvasRenderingContext2D, a: Annotation, scale: number): void {
  ctx.save();
  ctx.scale(scale, scale);
  ctx.strokeStyle = a.color;
  ctx.fillStyle = a.color;
  ctx.lineWidth = ANNOTATION_STROKE_WIDTH;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  const [first] = a.points;
  if (!first) {
    ctx.restore();
    return;
  }
  switch (a.tool) {
    case "pen": {
      ctx.beginPath();
      ctx.moveTo(first.x, first.y);
      for (const p of a.points.slice(1)) ctx.lineTo(p.x, p.y);
      ctx.stroke();
      break;
    }
    case "line": {
      const to = a.points.at(-1) ?? first;
      ctx.beginPath();
      ctx.moveTo(first.x, first.y);
      ctx.lineTo(to.x, to.y);
      ctx.stroke();
      break;
    }
    case "rect": {
      const r = normRect(first, a.points.at(-1) ?? first);
      ctx.strokeRect(r.x, r.y, r.w, r.h);
      break;
    }
    case "ellipse": {
      const r = normRect(first, a.points.at(-1) ?? first);
      ctx.beginPath();
      ctx.ellipse(r.x + r.w / 2, r.y + r.h / 2, r.w / 2, r.h / 2, 0, 0, Math.PI * 2);
      ctx.stroke();
      break;
    }
    case "text": {
      ctx.font = ANNOTATION_FONT;
      ctx.textBaseline = "top";
      ctx.fillText(a.text ?? "", first.x, first.y);
      break;
    }
  }
  ctx.restore();
}

// --- Feedback composition + delivery ---

/** The prompt for an annotated-screenshot send: the user's comment (or a
 * stock ask), where the preview was pointed, and the image paths via
 * `promptWithImages`' read-this-first phrasing. Newline-free like every
 * PTY-typed prompt — a literal newline inside the quoted argv drops zsh to a
 * PS2 continuation prompt. */
export function feedbackPrompt(comment: string, url: string, paths: string[]): string {
  const flat = comment.replaceAll(/\s*\n\s*/g, " ").trim();
  const goal = `${
    flat || "Please address the annotated feedback"
  } (annotated screenshot of the app preview at ${url})`;
  return promptWithImages(goal, paths);
}

/** What to type into the target session's PTY. Claude already running → the
 * bare prompt into its TUI input; plain shell → a `claude '<prompt>'` launch,
 * exactly like the new-task flow. Both end in `\r` to submit. */
export function feedbackPtyData(prompt: string, agentRunning: boolean): string {
  return agentRunning ? `${prompt}\r` : claudeCommand(prompt);
}

/** A PTY-live session the feedback can be typed into. Only `live` sessions
 * qualify: `term_write` reaches a PTY directly (no pane mount needed), but a
 * session that was never started has no PTY to reach — see "A pane has no PTY
 * until it is rendered" in apps/client/CLAUDE.md. */
export type SendTarget = {
  sessionId: string;
  label: string;
  agentRunning: boolean;
};

/** The live sessions in one folder — the preview pane belongs to a task
 * (folder), so its feedback goes to that task's own session, not a global
 * pick. Claude sessions first (feedback is usually for the agent already
 * working on that checkout); a folder with one live session needs no picker
 * at all. */
export function folderSendTargets(folder: Pick<FolderData, "sessions"> | undefined): SendTarget[] {
  if (!folder) return [];
  return folder.sessions
    .filter((s) => s.live)
    .map((s) => ({ sessionId: s.id, label: s.name, agentRunning: isAgent(s) }))
    .toSorted((a, b) => Number(b.agentRunning) - Number(a.agentRunning));
}
