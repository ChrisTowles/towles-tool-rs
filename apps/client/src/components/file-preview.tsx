import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ideReadFile } from "@/lib/ide";
import { monacoLanguageFor } from "@/lib/markdown-code";
import { loadMonaco } from "@/lib/monaco";

/**
 * A fenced code block, tokenized by Monaco.
 *
 * Uses the TextMate grammars and Dark Modern theme the editor already loads,
 * so a snippet in the preview is colored exactly like the same code in the
 * viewer — and it costs no extra dependency (Shiki would ship a second copy of
 * this same stack). Renders as plain text until tokenization resolves, and
 * stays plain if the language has no grammar.
 *
 * `dangerouslySetInnerHTML` is safe here specifically because Monaco's
 * colorizer HTML-escapes the source: verified against the running app with
 * `<script>` and `<img onerror=…>` payloads, both of which come back escaped.
 */
function FencedCode({ className, children }: { className?: string; children?: React.ReactNode }) {
  const source = String(children ?? "").replace(/\n$/, "");
  const language = monacoLanguageFor(className);
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    if (!language) return;
    let disposed = false;
    void (async () => {
      try {
        const monaco = await loadMonaco();
        const colored = await monaco.editor.colorize(source, language, { tabSize: 2 });
        if (!disposed) setHtml(colored);
      } catch {
        // No grammar, or the editor chunk failed to load — the plain text
        // below is a perfectly good fallback.
      }
    })();
    return () => {
      disposed = true;
    };
  }, [source, language]);

  if (!language || html == null) return <code className={className}>{source}</code>;
  return <code className={className} dangerouslySetInnerHTML={{ __html: html }} />;
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
      let read: Awaited<ReturnType<typeof ideReadFile>>;
      try {
        read = await ideReadFile(dir, path);
      } catch (e) {
        if (!disposed) setError(String(e));
        return;
      }
      if (disposed) return;
      if (read == null) {
        setError("not available in browser dev");
        return;
      }
      setContent(read.content);
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
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ code: FencedCode }}>
          {content}
        </ReactMarkdown>
      </div>
    </div>
  );
}
