import { describe, expect, it } from "vitest";
import type { FolderData, RepoData, SessionData } from "@/lib/agentboard";
import type { LaunchConfigStatus } from "@/lib/launch";
import {
  devServersOf,
  feedbackPrompt,
  feedbackPtyData,
  normRect,
  sendTargets,
} from "@/lib/preview";

const cfg = (over: Partial<LaunchConfigStatus>): LaunchConfigStatus => ({
  name: "dev",
  runtimeExecutable: "npm",
  runtimeArgs: ["run", "dev"],
  port: 1420,
  portListening: false,
  ...over,
});

describe("devServersOf", () => {
  it("maps port-bearing configs to dev servers and drops port-less ones", () => {
    const servers = devServersOf("app", "/repo", [
      cfg({ name: "vite", port: 1420, portListening: true }),
      cfg({ name: "storybook", port: null }),
    ]);
    expect(servers).toEqual([
      {
        key: "/repo\0vite",
        label: "app · vite :1420",
        url: "http://localhost:1420/",
        listening: true,
        folderDir: "/repo",
      },
    ]);
  });
});

describe("normRect", () => {
  it("normalizes any drag direction to a positive-size rect", () => {
    expect(normRect({ x: 10, y: 20 }, { x: 4, y: 50 })).toEqual({ x: 4, y: 20, w: 6, h: 30 });
    expect(normRect({ x: 0, y: 0 }, { x: 0, y: 0 })).toEqual({ x: 0, y: 0, w: 0, h: 0 });
  });
});

describe("feedbackPrompt", () => {
  it("names the preview URL and the image path, read-first phrased", () => {
    const p = feedbackPrompt("the header overlaps", "http://localhost:1425/", ["/tmp/a.png"]);
    expect(p).toBe(
      "the header overlaps (annotated screenshot of the app preview at http://localhost:1425/)" +
        " — Attached image — read it first, before anything else: /tmp/a.png",
    );
  });

  it("stays newline-free even when the comment has newlines (PTY-typed)", () => {
    const p = feedbackPrompt("line one\n  line two", "http://localhost:1/", ["/tmp/a.png"]);
    expect(p).not.toContain("\n");
    expect(p).toContain("line one line two");
  });

  it("falls back to a stock ask when the comment is empty", () => {
    expect(feedbackPrompt("  ", "http://localhost:1/", ["/x.png"])).toContain(
      "Please address the annotated feedback",
    );
  });
});

describe("feedbackPtyData", () => {
  it("types the bare prompt into a running Claude TUI", () => {
    expect(feedbackPtyData("fix it", true)).toBe("fix it\r");
  });

  it("launches claude with the quoted prompt in a plain shell", () => {
    expect(feedbackPtyData("fix it", false)).toBe("claude 'fix it'\r");
  });
});

const session = (over: Partial<SessionData>): SessionData =>
  ({
    id: "s1",
    name: "shell",
    createdAt: 0,
    live: false,
    unseen: false,
    agents: [],
    agentState: null,
    ...over,
  }) as SessionData;

const repo = (folders: FolderData[]): RepoData =>
  ({ key: "r", name: "towles-tool-rs", folders, needs: 0 }) as RepoData;

const folder = (sessions: SessionData[]): FolderData =>
  ({
    name: "primary",
    dir: "/repo",
    dirMissing: false,
    branch: "main",
    isWorktree: false,
    sessions,
  }) as unknown as FolderData;

describe("sendTargets", () => {
  it("keeps only PTY-live sessions and puts Claude sessions first", () => {
    const repos = [
      repo([
        folder([
          session({ id: "dead", live: false }),
          session({ id: "shell", live: true, name: "zsh" }),
          session({
            id: "agent",
            live: true,
            name: "claude",
            agentState: { status: "busy" } as SessionData["agentState"],
          }),
        ]),
      ]),
    ];
    const targets = sendTargets(repos);
    expect(targets.map((t) => t.sessionId)).toEqual(["agent", "shell"]);
    expect(targets[0].agentRunning).toBe(true);
    expect(targets[0].label).toBe("towles-tool-rs/primary · claude");
    expect(targets[0].folderDir).toBe("/repo");
  });
});
