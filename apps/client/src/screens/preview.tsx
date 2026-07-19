import { useEffect, useMemo, useRef, useState } from "react";
import {
  AppWindow,
  Circle,
  ExternalLink,
  Pen,
  RotateCw,
  Send,
  Slash,
  Square,
  Trash2,
  Type,
  Undo2,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Textarea } from "@/components/ui/textarea";
import { termWriteRetry, useAgentboardState } from "@/lib/agentboard";
import { errorMessage } from "@/lib/errors";
import { launchConfigs } from "@/lib/launch";
import { openExternalUrl } from "@/lib/open-url";
import {
  ANNOTATION_COLORS,
  ANNOTATION_FONT,
  type Annotation,
  type AnnotationTool,
  type DevServer,
  devServersOf,
  drawAnnotation,
  feedbackPrompt,
  feedbackPtyData,
  previewCapture,
  previewWriteFeedback,
  sendTargets,
} from "@/lib/preview";
import { uiAction } from "@/lib/ui-action";
import { cn } from "@/lib/utils";

/** The listening-state dot shared by the dev-server dropdown and the
 * empty-state launcher rows. */
function ServerDot({ listening }: { listening: boolean }) {
  return (
    <span
      className={cn("size-2 rounded-full", listening ? "bg-green-500" : "bg-muted-foreground/40")}
    />
  );
}

function pointFrom(e: React.PointerEvent<HTMLCanvasElement>) {
  return { x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY };
}

const TOOLS: { tool: AnnotationTool; icon: typeof Pen; title: string }[] = [
  { tool: "pen", icon: Pen, title: "Draw freehand" },
  { tool: "line", icon: Slash, title: "Line" },
  { tool: "rect", icon: Square, title: "Rectangle" },
  { tool: "ellipse", icon: Circle, title: "Ellipse" },
  { tool: "text", icon: Type, title: "Text note" },
];

/** Live preview of a running dev server with draw-on-the-page annotation,
 * sent back to a Claude session as an annotated screenshot — the Claude
 * Desktop page-preview flow, rebuilt on this app's own seams (launch.json
 * discovery, webview snapshot capture, PTY prompt delivery). */
export function PreviewScreen() {
  const state = useAgentboardState();

  // --- URL / navigation ---
  const [url, setUrl] = useState("");
  const [input, setInput] = useState("");
  const [frameKey, setFrameKey] = useState(0);
  const [servers, setServers] = useState<DevServer[]>([]);

  // --- annotation ---
  const [tool, setTool] = useState<AnnotationTool | null>(null);
  const [color, setColor] = useState<string>(ANNOTATION_COLORS[0]);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [draft, setDraft] = useState<Annotation | null>(null);
  const [textDraft, setTextDraft] = useState<{ x: number; y: number; value: string } | null>(null);

  // --- send dialog ---
  const [capture, setCapture] = useState<string | null>(null);
  const [comment, setComment] = useState("");
  const [targetId, setTargetId] = useState<string | null>(null);
  const [sending, setSending] = useState(false);

  const surfaceRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const draftRef = useRef<Annotation | null>(null);
  const redrawRef = useRef<() => void>(() => {});
  const stateRef = useRef(state);
  stateRef.current = state;

  const targets = useMemo(() => sendTargets(state.repos), [state.repos]);

  // Discover dev servers by probing each tracked folder's launch.json. Keyed
  // on the folder-dir set plus a slow interval, not the repos array identity —
  // state snapshots arrive on every poll, and each probe is a TCP connect per
  // port; the interval is what notices a launch.json appearing or a server
  // starting/stopping without a folder-set change (loopback connects are
  // effectively free at this cadence, and it keeps the status dots honest).
  const dirsKey = state.repos
    .flatMap((r) => r.folders.map((f) => f.dir))
    .toSorted()
    .join("\n");
  useEffect(() => {
    let cancelled = false;
    const probe = async () => {
      // Folders are independent, so probe them concurrently — one slow port
      // connect can't serialize the rest behind it.
      const folders = stateRef.current.repos.flatMap((repo) =>
        repo.folders.filter((f) => !f.dirMissing).map((folder) => ({ repo, folder })),
      );
      const found = (
        await Promise.all(
          folders.map(async ({ repo, folder }) => {
            const res = await launchConfigs(folder.dir);
            return res.isOk() ? devServersOf(repo.name, folder.dir, res.value) : [];
          }),
        )
      ).flat();
      if (!cancelled) setServers(found);
    };
    void probe();
    const timer = setInterval(() => void probe(), 15_000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [dirsKey]);

  function navigate(next: string, source: "manual" | "config") {
    const withScheme = /^[a-z]+:\/\//i.test(next) ? next : `http://${next}`;
    setUrl(withScheme);
    setInput(withScheme);
    setFrameKey((k) => k + 1);
    uiAction("preview.navigate", "preview", source);
  }

  function redraw() {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const dpr = window.devicePixelRatio || 1;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    for (const a of annotations) drawAnnotation(ctx, a, dpr);
    if (draft) drawAnnotation(ctx, draft, dpr);
  }
  // A resize fires from the ResizeObserver, which is installed once at mount —
  // its closure would otherwise capture the empty mount-render `redraw` and
  // repaint the canvas blank after a resize. Route both the resize and the
  // reactive redraw through the always-current function.
  redrawRef.current = redraw;

  // --- canvas sizing ---
  useEffect(() => {
    const canvas = canvasRef.current;
    const surface = surfaceRef.current;
    if (!canvas || !surface) return;
    const resize = () => {
      const dpr = window.devicePixelRatio || 1;
      const r = surface.getBoundingClientRect();
      canvas.width = Math.round(r.width * dpr);
      canvas.height = Math.round(r.height * dpr);
      redrawRef.current();
    };
    const ro = new ResizeObserver(resize);
    ro.observe(surface);
    resize();
    return () => ro.disconnect();
  }, []);

  // Redraw on every annotation change (the canvas is imperative).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(redraw, [annotations, draft]);

  // Escape backs out one level: draft → text note → tool.
  useEffect(() => {
    if (!tool) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (textDraft) setTextDraft(null);
      else if (draftRef.current) {
        draftRef.current = null;
        setDraft(null);
      } else setTool(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [tool, draft, textDraft]);

  function onPointerDown(e: React.PointerEvent<HTMLCanvasElement>) {
    if (!tool || e.button !== 0) return;
    const p = pointFrom(e);
    if (tool === "text") {
      commitTextDraft();
      setTextDraft({ x: p.x, y: p.y, value: "" });
      return;
    }
    // Throws NotFoundError if the pointer was already released (a fast
    // click can race the capture request) — losing capture just means an
    // off-canvas drag stops extending the stroke, not worth crashing over.
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // ignore
    }
    draftRef.current = { tool, color, points: [p] };
    setDraft(draftRef.current);
  }

  // The in-progress stroke lives in a ref, with `draft` state as its render
  // mirror: pointermove fires faster than React re-renders, and a handler
  // reading the state closure would extend a stale stroke and drop points.
  function onPointerMove(e: React.PointerEvent<HTMLCanvasElement>) {
    const d = draftRef.current;
    if (!d) return;
    const p = pointFrom(e);
    draftRef.current =
      d.tool === "pen" ? { ...d, points: [...d.points, p] } : { ...d, points: [d.points[0], p] };
    setDraft(draftRef.current);
  }

  function onPointerUp() {
    const d = draftRef.current;
    if (!d) return;
    draftRef.current = null;
    setDraft(null);
    setAnnotations((all) => [...all, d]);
  }

  // Two independent top-level setStates, never a setAnnotations nested inside
  // the setTextDraft updater — an updater must be pure, and StrictMode's dev
  // double-invoke would otherwise enqueue the note twice. Reading `textDraft`
  // from the render closure is safe: every caller is a fresh per-render
  // handler (blur/Enter/tool-switch/next-click), never a mount-bound effect.
  function commitTextDraft() {
    const td = textDraft;
    if (td && td.value.trim()) {
      setAnnotations((all) => [
        ...all,
        { tool: "text", color, points: [{ x: td.x, y: td.y }], text: td.value.trim() },
      ]);
    }
    setTextDraft(null);
  }

  function clearAnnotations() {
    setAnnotations([]);
    draftRef.current = null;
    setDraft(null);
    setTextDraft(null);
    uiAction("preview.annotate.clear", "preview");
  }

  function selectTool(next: AnnotationTool | null) {
    // A tool switch mid-stroke abandons the stroke — committing a half-drawn
    // shape the user was escaping from would be worse.
    draftRef.current = null;
    setDraft(null);
    commitTextDraft();
    setTool(next);
  }

  // --- capture + send ---
  async function openSendDialog() {
    const surface = surfaceRef.current;
    if (!surface) return;
    commitTextDraft();
    uiAction("preview.feedback.capture", "preview");
    const r = surface.getBoundingClientRect();
    const res = await previewCapture({
      x: r.x,
      y: r.y,
      width: r.width,
      height: r.height,
      devicePixelRatio: window.devicePixelRatio || 1,
    });
    res.match({
      ok: (png) => {
        setCapture(png);
        setComment("");
        const server = servers.find((s) => s.url === url);
        const preferred = targets.find((t) => t.folderDir === server?.folderDir) ?? targets.at(0);
        setTargetId(preferred?.sessionId ?? null);
      },
      err: (e) => toast.error(`Capture failed: ${errorMessage(e)}`),
    });
  }

  async function sendFeedback() {
    if (!capture) return;
    const target = targets.find((t) => t.sessionId === targetId);
    if (!target) {
      // The chosen session can die between opening the dialog and clicking
      // Send (targets is recomputed from every agentboard poll) — say so
      // rather than no-op'ing on the click.
      toast.error("That session is no longer running — pick another.");
      return;
    }
    setSending(true);
    const written = await previewWriteFeedback(target.repoName, [
      { mime: "image/png", dataBase64: capture },
    ]);
    if (written.isErr()) {
      setSending(false);
      uiAction("preview.feedback.send", "preview", "err");
      toast.error(`Send failed: ${errorMessage(written.error)}`);
      return;
    }
    const prompt = feedbackPrompt(comment, url, written.value);
    const sent = await termWriteRetry(
      target.sessionId,
      feedbackPtyData(prompt, target.agentRunning),
    );
    setSending(false);
    sent.match({
      ok: () => {
        uiAction("preview.feedback.send", "preview", "ok");
        toast.success(`Sent to ${target.label}`);
        setCapture(null);
        setAnnotations([]);
        setTool(null);
      },
      err: (e) => {
        uiAction("preview.feedback.send", "preview", "err");
        toast.error(`Send failed: ${errorMessage(e)}`);
      },
    });
  }

  // The surface div renders unconditionally, so its ref is set on every frame
  // after mount; a URL is the only real gate. openSendDialog re-checks the ref.
  const canSend = url !== "";

  return (
    <div className="flex h-full flex-col">
      {/* URL bar */}
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-border bg-card px-3">
        {servers.length > 0 && (
          <Select
            value={servers.find((s) => s.url === url)?.key ?? ""}
            onValueChange={(key) => {
              const s = servers.find((x) => x.key === key);
              if (s) navigate(s.url, "config");
            }}
          >
            <SelectTrigger size="sm" className="w-56">
              <SelectValue placeholder="Dev servers" />
            </SelectTrigger>
            <SelectContent>
              {servers.map((s) => (
                <SelectItem key={s.key} value={s.key}>
                  <ServerDot listening={s.listening} />
                  {s.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <Input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && input.trim()) navigate(input.trim(), "manual");
          }}
          placeholder="http://localhost:<port>/ — or pick a detected dev server"
          className="h-7 flex-1 font-mono text-xs"
        />
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          title="Reload"
          disabled={!url}
          onClick={() => {
            setFrameKey((k) => k + 1);
            uiAction("preview.reload", "preview");
          }}
        >
          <RotateCw />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          title="Open in browser"
          disabled={!url}
          onClick={() => {
            uiAction("preview.open_external", "preview");
            void openExternalUrl(url);
          }}
        >
          <ExternalLink />
        </Button>
      </div>

      {/* Preview surface: iframe + annotation canvas */}
      <div ref={surfaceRef} className="relative min-h-0 flex-1 overflow-hidden bg-background">
        {url ? (
          /* Unsandboxed by intent: the previewed page is the user's own local
           * dev server, and it needs scripts *and* its own origin (HMR
           * websockets, storage) to function — and a sandbox granting both is
           * the combination the lint itself flags as useless. The rule's
           * escape scenario (sandboxed content same-origin with the host)
           * doesn't apply: localhost:<port> is a different origin than the
           * app shell. */
          // oxlint-disable-next-line react/iframe-missing-sandbox
          <iframe
            key={frameKey}
            src={url}
            title="Dev server preview"
            className="absolute inset-0 h-full w-full border-0 bg-white"
          />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-center">
            <AppWindow className="size-8 text-muted-foreground/60" />
            <div className="text-sm text-muted-foreground">
              Point the preview at a running dev server
            </div>
            {servers.length > 0 ? (
              <div className="flex flex-col gap-1.5">
                {servers.map((s) => (
                  <button
                    key={s.key}
                    type="button"
                    className="flex items-center gap-2 rounded-md border border-border bg-card px-3 py-1.5 text-left text-xs hover:bg-accent"
                    onClick={() => navigate(s.url, "config")}
                  >
                    <ServerDot listening={s.listening} />
                    <span className="font-mono">{s.label}</span>
                    {!s.listening && <span className="text-muted-foreground/60">not running</span>}
                  </button>
                ))}
              </div>
            ) : (
              <div className="max-w-sm text-xs text-muted-foreground/60">
                No <span className="font-mono">.claude/launch.json</span> configs found in tracked
                repos — enter a URL above.
              </div>
            )}
          </div>
        )}
        <canvas
          ref={canvasRef}
          className={cn(
            "absolute inset-0 h-full w-full",
            tool === "text" ? "cursor-text" : "cursor-crosshair",
          )}
          style={{ pointerEvents: tool ? "auto" : "none" }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
        />
        {textDraft && (
          <input
            autoFocus
            value={textDraft.value}
            onChange={(e) => setTextDraft({ ...textDraft, value: e.target.value })}
            onBlur={commitTextDraft}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitTextDraft();
            }}
            className="absolute z-10 border border-dashed bg-transparent outline-none"
            style={{
              left: textDraft.x,
              top: textDraft.y,
              color,
              borderColor: color,
              font: ANNOTATION_FONT,
              minWidth: 120,
            }}
          />
        )}
      </div>

      {/* Annotation toolbar */}
      <div className="flex shrink-0 items-center gap-1 border-t border-border bg-card px-3 py-1.5">
        {TOOLS.map(({ tool: t, icon: Icon, title }) => (
          <Button
            key={t}
            variant="ghost"
            size="icon"
            title={title}
            disabled={!url}
            className={cn("size-7", tool === t && "bg-accent text-foreground")}
            onClick={() => selectTool(tool === t ? null : t)}
          >
            <Icon />
          </Button>
        ))}
        <Separator orientation="vertical" className="mx-1 h-5" />
        {ANNOTATION_COLORS.map((c) => (
          <button
            key={c}
            type="button"
            title="Ink color"
            className={cn(
              "size-4 rounded-full border border-border",
              color === c && "ring-2 ring-ring ring-offset-1 ring-offset-card",
            )}
            style={{ backgroundColor: c }}
            onClick={() => setColor(c)}
          />
        ))}
        <Separator orientation="vertical" className="mx-1 h-5" />
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          title="Undo last mark"
          disabled={annotations.length === 0}
          onClick={() => setAnnotations((all) => all.slice(0, -1))}
        >
          <Undo2 />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          title="Clear all marks"
          disabled={annotations.length === 0 && !draft}
          onClick={clearAnnotations}
        >
          <Trash2 />
        </Button>
        <div className="ml-auto flex items-center gap-2">
          {(tool || annotations.length > 0) && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                clearAnnotations();
                selectTool(null);
                uiAction("preview.annotate.cancel", "preview");
              }}
            >
              Cancel
            </Button>
          )}
          <Button size="sm" disabled={!canSend} onClick={() => void openSendDialog()}>
            <Send /> Send to agent
          </Button>
        </div>
      </div>

      {/* Capture → comment → target dialog */}
      <Dialog open={capture != null} onOpenChange={(open) => !open && setCapture(null)}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Send annotated feedback</DialogTitle>
            <DialogDescription>
              The screenshot below (with your markup) is staged as a file and its path typed into
              the session&apos;s prompt.
            </DialogDescription>
          </DialogHeader>
          {capture && (
            <img
              src={`data:image/png;base64,${capture}`}
              alt="Annotated preview capture"
              className="max-h-64 w-full rounded-md border border-border object-contain"
            />
          )}
          <Textarea
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            placeholder="What should the agent do about it?"
            rows={2}
          />
          {targets.length > 0 ? (
            <Select value={targetId ?? ""} onValueChange={setTargetId}>
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Send to session…" />
              </SelectTrigger>
              <SelectContent>
                {targets.map((t) => (
                  <SelectItem key={t.sessionId} value={t.sessionId}>
                    <span
                      className={cn(
                        "font-mono text-xs",
                        t.agentRunning ? "text-violet-500" : "text-muted-foreground/60",
                      )}
                    >
                      {t.agentRunning ? "✦" : "❯"}
                    </span>
                    {t.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : (
            <div className="text-xs text-muted-foreground">
              No live sessions — start one in Agentboard first.
            </div>
          )}
          <DialogFooter>
            <Button variant="ghost" onClick={() => setCapture(null)}>
              Cancel
            </Button>
            <Button
              disabled={sending || !targetId || targets.length === 0}
              onClick={() => void sendFeedback()}
            >
              {sending ? "Sending…" : "Send"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
