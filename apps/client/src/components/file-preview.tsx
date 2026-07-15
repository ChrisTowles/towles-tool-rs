import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ideReadFile } from "@/lib/ide";

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
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
      </div>
    </div>
  );
}
