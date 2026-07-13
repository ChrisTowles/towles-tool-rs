import { Fragment, type ReactNode } from "react";
import { mentionLabel, parseMrkdwn, type MrkdwnNode } from "@/lib/mrkdwn";
import { openExternalUrl } from "@/lib/open-url";

/**
 * Render Slack mrkdwn message text (see {@link parseMrkdwn}) as React. Links go
 * to the OS browser via {@link openExternalUrl} — never navigating the webview —
 * and `<@U…>` mentions resolve to names through {@link mentionLabel} using the
 * watched user's id/name. Returns a fragment so the caller's bubble keeps
 * control of layout (and its `whitespace-pre-wrap`, which preserves newlines in
 * text nodes).
 */
export function MrkdwnText({
  text,
  watchUserId,
  watchName,
}: {
  text: string;
  watchUserId?: string;
  watchName?: string;
}) {
  return <>{renderNodes(parseMrkdwn(text), { watchUserId, watchName })}</>;
}

type MentionOpts = { watchUserId?: string; watchName?: string };

function renderNodes(nodes: MrkdwnNode[], opts: MentionOpts): ReactNode {
  return nodes.map((node, i) => <Fragment key={i}>{renderNode(node, opts)}</Fragment>);
}

const MENTION_CLASS =
  "rounded bg-violet-500/15 px-1 font-medium text-violet-700 dark:text-violet-300";

function renderNode(node: MrkdwnNode, opts: MentionOpts): ReactNode {
  switch (node.type) {
    case "text":
      return node.value;
    case "strong":
      return <strong className="font-semibold">{renderNodes(node.children, opts)}</strong>;
    case "em":
      return <em className="italic">{renderNodes(node.children, opts)}</em>;
    case "del":
      return <del className="line-through opacity-80">{renderNodes(node.children, opts)}</del>;
    case "code":
      return (
        <code className="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]">{node.value}</code>
      );
    case "pre":
      return (
        <pre className="my-1 overflow-x-auto rounded bg-muted p-2 font-mono text-[0.85em]">
          {node.value}
        </pre>
      );
    case "link":
      return (
        <a
          href={node.url}
          onClick={(e) => {
            e.preventDefault();
            void openExternalUrl(node.url);
          }}
          className="text-violet-700 underline underline-offset-2 hover:text-violet-500 dark:text-violet-300"
        >
          {node.label}
        </a>
      );
    case "user":
      return <span className={MENTION_CLASS}>{mentionLabel(node.id, node.label, opts)}</span>;
    case "channel":
      return <span className={MENTION_CLASS}>#{node.label}</span>;
    case "broadcast":
      return <span className={MENTION_CLASS}>{node.label}</span>;
  }
}
