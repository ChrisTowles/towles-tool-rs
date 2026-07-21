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
  Type,
} from "lucide-react";
import { toast } from "sonner";
import { Glyph, IconBtn, PanePlaceholder } from "@/components/agentboard-bits";
import { PaneChrome, PaneLens } from "@/components/pane-chrome";
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
import { type FolderData, termWriteRetry } from "@/lib/agentboard";
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
  folderSendTargets,
  previewCapture,
  previewWriteFeedback,
} from "@/lib/preview";
import { uiAction } from "@/lib/ui-action";
import { cn } from "@/lib/utils";

const TOOLS: { tool: AnnotationTool; icon: typeof Pen; title: string }[] = [
  { tool: "pen", icon: Pen, title: "Draw freehand" },
  { tool: "line", icon: Slash, title: "Line" },
  { tool: "rect", icon: Square, title: "Rectangle" },
  { tool: "ellipse", icon: Circle, title: "Ellipse" },
  { tool: "text", icon: Type, title: "Text note" },
];

function pointFrom(e: React.PointerEvent<HTMLCanvasElement>) {
  return { x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY };
}

/** A task's live dev server embedded beside its terminals, with draw-on-page
 * annotation sent back to that task's own Claude session as an annotated
 * screenshot. A folder pane (like diff/files): scoped to one checkout, so the
 * dev server comes from *this* folder's `.claude/launch.json` and the feedback
 * targets *this* folder's session — no global URL bar or session picker. */
export function PreviewPane({
  folder,
  focused,
  onClose,
}: {
  /** The checkout this pane previews; undefined when it left the rail. */
  folder: FolderData | undefined;
  /** This pane is the one the user last clicked into — see the focus-ring
   * rule in `screens/agentboard.tsx`'s `focusedPaneId`. */
  focused: boolean;
  /** Removes the pane from its window. */
  onClose: () => void;
}) {
  const dir = folder?.dir;

  // --- URL / navigation ---
  const [url, setUrl] = useState("");
  const [input, setInput] = useState("");
  const [frameKey, setFrameKey] = useState(0);
  const [servers, setServers] = useState<DevServer[]>([]);

  // --- annotation ---
  const [tool, setTool] = useState<AnnotationTool | null>(null);
  const [color, setColor] = useState<string>(ANNOTATION_COLORS[0]);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
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

  const targets = useMemo(() => folderSendTargets(folder), [folder]);

  // Discover this folder's dev server(s) from its launch.json — probing on
  // mount, on folder change, and a slow interval (each probe is a TCP connect
  // per port; the interval catches a server starting/stopping). Auto-load the
  // first listening one so the pane opens showing something.
  //
  // Unlike diff-pane (which refetches off `statsKey`, a value the shared 1.5s
  // agentboard poll already bumps), this owns a timer per open pane. That's a
  // deliberate divergence: launch.json + port status isn't in the agentboard
  // snapshot, and a handful of panes probing every 15s is cheap. If dev-server
  // status ever lands on `FolderData`, key off it and drop this timer.
  useEffect(() => {
    if (!dir) return;
    let cancelled = false;
    const probe = async () => {
      const res = await launchConfigs(dir);
      if (cancelled) return;
      const found = res.isOk() ? devServersOf(folder?.name ?? "", dir, res.value) : [];
      setServers(found);
      setUrl((cur) => {
        if (cur) return cur;
        const auto = found.find((s) => s.listening) ?? found[0];
        return auto?.url ?? cur;
      });
    };
    void probe();
    const timer = setInterval(() => void probe(), 15_000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
    // folder?.name only feeds labels; dir is the identity that matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir]);

  function navigate(next: string, source: "manual" | "config") {
    const withScheme = /^[a-z]+:\/\//i.test(next) ? next : `http://${next}`;
    setUrl(withScheme);
    setInput(withScheme);
    setFrameKey((k) => k + 1);
    uiAction("preview.navigate", "agentboard", source);
  }

  // --- canvas draw model ---
  // The in-progress stroke lives only in `draftRef`, never React state: it's
  // the authoritative value the pointer handlers mutate and paint imperatively
  // (a `setDraft` per pointermove would re-render the whole pane every move for
  // no benefit — and reading it back from state would drop points, since
  // several moves can fire before a render lands). Committed `annotations` are
  // state and repaint via the effect below.
  function redraw() {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const dpr = window.devicePixelRatio || 1;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    for (const a of annotations) drawAnnotation(ctx, a, dpr);
    if (draftRef.current) drawAnnotation(ctx, draftRef.current, dpr);
  }
  redrawRef.current = redraw;

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

  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(redraw, [annotations]);

  // Abandon the in-progress stroke and wipe it off the canvas — the shared
  // "escape the current draft" used by Escape, tool-switch, and clear.
  function discardDraft() {
    draftRef.current = null;
    redrawRef.current();
  }

  useEffect(() => {
    if (!tool) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (textDraft) setTextDraft(null);
      else if (draftRef.current) discardDraft();
      else setTool(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [tool, textDraft]);

  function onPointerDown(e: React.PointerEvent<HTMLCanvasElement>) {
    if (!tool || e.button !== 0) return;
    const p = pointFrom(e);
    if (tool === "text") {
      commitTextDraft();
      setTextDraft({ x: p.x, y: p.y, value: "" });
      return;
    }
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // ignore
    }
    draftRef.current = { tool, color, points: [p] };
    redrawRef.current();
  }

  function onPointerMove(e: React.PointerEvent<HTMLCanvasElement>) {
    const d = draftRef.current;
    if (!d) return;
    const p = pointFrom(e);
    draftRef.current =
      d.tool === "pen" ? { ...d, points: [...d.points, p] } : { ...d, points: [d.points[0], p] };
    redrawRef.current();
  }

  function onPointerUp() {
    const d = draftRef.current;
    if (!d) return;
    draftRef.current = null;
    // Promoting to state re-renders and the effect repaints it as committed —
    // the imperative frame stays on screen until then, so no flicker.
    setAnnotations((all) => [...all, d]);
  }

  // Two independent top-level setStates — never setAnnotations nested inside
  // the setTextDraft updater (StrictMode double-invoke would duplicate the
  // note). Reading `textDraft` from the render closure is safe: callers are
  // all fresh per-render handlers.
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
    draftRef.current = null;
    setTextDraft(null);
    setAnnotations([]); // effect repaints the now-empty canvas
  }

  function selectTool(next: AnnotationTool | null) {
    commitTextDraft();
    discardDraft();
    setTool(next);
  }

  async function openSendDialog() {
    const surface = surfaceRef.current;
    if (!surface) return;
    commitTextDraft();
    uiAction("preview.feedback.capture", "agentboard");
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
        setTargetId(targets.at(0)?.sessionId ?? null);
      },
      err: (e) => toast.error(`Capture failed: ${errorMessage(e)}`),
    });
  }

  async function sendFeedback() {
    if (!capture || !dir) return;
    const target = targets.find((t) => t.sessionId === targetId);
    if (!target) {
      toast.error("That session is no longer running — pick another.");
      return;
    }
    setSending(true);
    const written = await previewWriteFeedback(folder?.name ?? "preview", [
      { mime: "image/png", dataBase64: capture },
    ]);
    if (written.isErr()) {
      setSending(false);
      uiAction("preview.feedback.send", "agentboard", "err");
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
        uiAction("preview.feedback.send", "agentboard", "ok");
        toast.success(`Sent to ${target.label}`);
        setCapture(null);
        setAnnotations([]);
        setTool(null);
      },
      err: (e) => {
        uiAction("preview.feedback.send", "agentboard", "err");
        toast.error(`Send failed: ${errorMessage(e)}`);
      },
    });
  }

  if (!folder) return <PanePlaceholder label="folder gone" focused={focused} onRemove={onClose} />;

  return (
    <div
      className={cn(
        "flex h-full flex-col overflow-hidden rounded-lg border bg-card",
        focused && "border-violet-500/60",
      )}
    >
      {/* Header: title + URL/server + reload/external + close */}
      <PaneChrome
        lens={<PaneLens kind="web" />}
        controls={
          <>
            {servers.length > 0 && (
              <Select
                value={servers.find((s) => s.url === url)?.key ?? ""}
                onValueChange={(key) => {
                  const s = servers.find((x) => x.key === key);
                  if (s) navigate(s.url, "config");
                }}
              >
                <SelectTrigger size="xs" className="w-40 text-[11px]">
                  <SelectValue placeholder="Dev server" />
                </SelectTrigger>
                <SelectContent>
                  {servers.map((s) => (
                    <SelectItem key={s.key} value={s.key}>
                      <span
                        className={cn(
                          "size-2 rounded-full",
                          s.listening ? "bg-green-500" : "bg-muted-foreground/40",
                        )}
                      />
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
              placeholder="http://localhost:<port>/"
              className="h-6 min-w-0 flex-1 font-mono text-[11px]"
            />
          </>
        }
        actions={
          <>
            <IconBtn
              title="reload preview"
              disabled={!url}
              className="hover:text-sky-500"
              onClick={() => {
                setFrameKey((k) => k + 1);
                uiAction("preview.reload", "agentboard");
              }}
            >
              <RotateCw className="size-3" />
            </IconBtn>
            <IconBtn
              title="open in browser"
              disabled={!url}
              className="hover:text-sky-500"
              onClick={() => {
                uiAction("preview.open_external", "agentboard");
                void openExternalUrl(url);
              }}
            >
              <ExternalLink className="size-3" />
            </IconBtn>
            <IconBtn
              title="remove pane (preview stays a click away on the folder)"
              className="hover:text-red-500"
              onClick={onClose}
            >
              ⊟
            </IconBtn>
          </>
        }
      />

      {/* Surface: iframe + annotation canvas */}
      <div ref={surfaceRef} className="relative min-h-0 flex-1 overflow-hidden bg-background">
        {url ? (
          /* Unsandboxed by intent: the previewed page is the user's own local
           * dev server and needs scripts + its own origin (HMR, storage) to
           * function — the combination the sandbox lint flags as useless. */
          // oxlint-disable-next-line react/iframe-missing-sandbox
          <iframe
            key={frameKey}
            src={url}
            title="Dev server preview"
            className="absolute inset-0 h-full w-full border-0 bg-white"
          />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-2 px-4 text-center">
            <AppWindow className="size-6 text-muted-foreground/60" />
            <div className="text-xs text-muted-foreground">
              No dev server found in this checkout&apos;s{" "}
              <span className="font-mono">.claude/launch.json</span> — enter a URL above.
            </div>
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
              minWidth: 100,
            }}
          />
        )}
      </div>

      {/* Annotation toolbar */}
      <div className="flex shrink-0 items-center gap-1 border-t bg-card px-2 py-1">
        {TOOLS.map(({ tool: t, icon: Icon, title }) => (
          <Button
            key={t}
            variant="ghost"
            size="icon"
            title={title}
            disabled={!url}
            className={cn("size-6", tool === t && "bg-accent text-foreground")}
            onClick={() => selectTool(tool === t ? null : t)}
          >
            <Icon className="size-3.5" />
          </Button>
        ))}
        <Separator orientation="vertical" className="mx-0.5 h-4" />
        {ANNOTATION_COLORS.map((c) => (
          <button
            key={c}
            type="button"
            title="Ink color"
            className={cn(
              "size-3.5 rounded-full border border-border",
              color === c && "ring-2 ring-ring ring-offset-1 ring-offset-card",
            )}
            style={{ backgroundColor: c }}
            onClick={() => setColor(c)}
          />
        ))}
        <div className="ml-auto flex items-center gap-1.5">
          {annotations.length > 0 && (
            <Button variant="ghost" size="xs" onClick={clearAnnotations}>
              Clear
            </Button>
          )}
          <Button size="xs" disabled={!url} onClick={() => void openSendDialog()}>
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
          {targets.length > 1 ? (
            <Select value={targetId ?? ""} onValueChange={setTargetId}>
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Send to session…" />
              </SelectTrigger>
              <SelectContent>
                {targets.map((t) => (
                  <SelectItem key={t.sessionId} value={t.sessionId}>
                    <Glyph agent={t.agentRunning} />
                    {t.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : targets.length === 0 ? (
            <div className="text-xs text-muted-foreground">
              No live session in this checkout — start one in the rail first.
            </div>
          ) : null}
          <DialogFooter>
            <Button variant="ghost" onClick={() => setCapture(null)}>
              Cancel
            </Button>
            <Button disabled={sending || !targetId} onClick={() => void sendFeedback()}>
              {sending ? "Sending…" : "Send"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
