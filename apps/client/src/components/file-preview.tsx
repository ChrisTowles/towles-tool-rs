import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ideReadFile } from "@/lib/ide";
import { NotInTauri } from "@/lib/errors";
import { monacoLanguageFor } from "@/lib/markdown-code";
import { loadedMonaco } from "@/lib/monaco";

/**
 * Tokenized fences, keyed by language + source. Colorizing is a few ms per
 * block, but a preview remounts on every file switch and preview toggle, so
 * without this a 30-fence README re-tokenizes all 30 each time you navigate
 * back to it. Bounded because the values are rendered HTML, not just text.
 */
const COLORIZED = new Map<string, string>();
const COLORIZED_MAX = 200;

/**
 * A fenced code block, tokenized by Monaco.
 *
 * Uses the TextMate grammars and Dark Modern theme the editor already loads,
 * so a snippet in the preview is colored exactly like the same code in the
 * viewer — and it costs no extra dependency (Shiki would ship a second copy of
 * this same stack). Deliberately never *starts* that load: the preview only
 * ever renders beside a CodeViewer, which boots Monaco anyway, and a preview
 * has no business paying a multi-megabyte bootstrap to color a snippet. Plain
 * text is the fallback whenever it isn't up.
 *
 * `dangerouslySetInnerHTML` is safe here specifically because Monaco's
 * colorizer HTML-escapes the source: verified against the running app with
 * `<script>` and `<img onerror=…>` payloads, both of which come back escaped.
 */
function FencedCode({
  language,
  source,
  className,
}: {
  language: string;
  source: string;
  className?: string;
}) {
  // A NUL can't occur in a fence, so it separates the two halves
  // unambiguously. Written as an escape rather than typed literally — a raw
  // NUL byte in the source makes git treat this whole file as binary and
  // stop diffing it.
  const key = `${language}\u0000${source}`;
  // Keyed state rather than a bare string: react-markdown reuses this
  // component instance across content changes, so a plain `html` would keep
  // painting the *previous* fence's tokens — permanently in the case where
  // the new key is already cached and the effect below returns early.
  // Reading the cache during render also paints a revisited block colored on
  // the first frame, with no effect and no second render.
  const [done, setDone] = useState<{ key: string; html: string } | null>(null);
  const html = done?.key === key ? done.html : (COLORIZED.get(key) ?? null);

  useEffect(() => {
    if (COLORIZED.has(key)) return;
    let disposed = false;
    void (async () => {
      try {
        const pending = loadedMonaco();
        if (!pending) return;
        const monaco = await pending;
        const colored = await monaco.editor.colorize(source, language, { tabSize: 2 });
        if (COLORIZED.size >= COLORIZED_MAX) COLORIZED.clear();
        COLORIZED.set(key, colored);
        if (!disposed) setDone({ key, html: colored });
      } catch {
        // No grammar for this fence — plain text below is a fine fallback.
      }
    })();
    return () => {
      disposed = true;
    };
  }, [key, language, source]);

  if (html == null) return <code className={className}>{source}</code>;
  return <code className={className} dangerouslySetInnerHTML={{ __html: html }} />;
}

/**
 * react-markdown routes *inline* `code` through this too, and prose has far
 * more inline spans than fences — so resolve the language first and only mount
 * the stateful highlighter for a fence that can actually be colored.
 */
function MarkdownCode({ className, children }: { className?: string; children?: React.ReactNode }) {
  const language = monacoLanguageFor(className);
  const source = String(children ?? "").replace(/\n$/, "");
  if (!language) return <code className={className}>{source}</code>;
  return <FencedCode language={language} source={source} className={className} />;
}

/** Extensions the file viewer knows how to render a live preview for. */
export type PreviewKind = "markdown" | "html";

export function previewKindFor(path: string): PreviewKind | null {
  const ext = path.split(".").pop()?.toLowerCase();
  if (ext === "md" || ext === "markdown") return "markdown";
  if (ext === "html" || ext === "htm") return "html";
  return null;
}

/**
 * Read-only render of a Markdown or HTML file's current-on-disk content —
 * the split pane's right side. HTML renders in a script-less sandboxed
 * iframe so a previewed file can never run script in the app's origin.
 */
export function FilePreview({ dir, path, kind }: { dir: string; path: string; kind: PreviewKind }) {
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    setContent(null);
    setError(null);
    void (async () => {
      const read = await ideReadFile(dir, path);
      if (disposed) return;
      read.match({
        ok: (file) => setContent(file.content),
        err: (e) => setError(NotInTauri.is(e) ? "not available in browser dev" : e.message),
      });
    })();
    return () => {
      disposed = true;
    };
  }, [dir, path]);

  if (error) {
    return <p className="p-3 text-sm text-muted-foreground">{error}</p>;
  }
  if (content == null) {
    return <p className="p-3 text-sm text-muted-foreground">Loading…</p>;
  }
  if (kind === "html") {
    return (
      <iframe
        title={path}
        srcDoc={content}
        sandbox=""
        className="h-full w-full border-0 bg-white"
      />
    );
  }
  return (
    <div className="h-full min-w-0 flex-1 overflow-y-auto px-4 py-3">
      <div className="prose prose-sm dark:prose-invert max-w-none">
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ code: MarkdownCode }}>
          {content}
        </ReactMarkdown>
      </div>
    </div>
  );
}
