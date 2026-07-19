import { describe, expect, it } from "vitest";
import { launchAction, launchCommand, type LaunchConfigStatus } from "@/lib/launch";

const cfg = (over: Partial<LaunchConfigStatus>): LaunchConfigStatus => ({
  name: "blog",
  runtimeExecutable: "pnpm",
  runtimeArgs: [],
  port: 3000,
  portListening: false,
  sessionId: null,
  ...over,
});

describe("launchCommand", () => {
  it("renders the blog fixture's config as typed, unquoted", () => {
    // The real Claude Desktop launch.json this feature exists to run.
    expect(
      launchCommand({
        runtimeExecutable: "pnpm",
        runtimeArgs: ["--filter", "@chris-towles/blog", "dev"],
      }),
    ).toBe("pnpm --filter @chris-towles/blog dev");
  });

  it("quotes only tokens the shell would mangle", () => {
    expect(
      launchCommand({ runtimeExecutable: "node", runtimeArgs: ["my server.js", "--port", "3000"] }),
    ).toBe("node 'my server.js' --port 3000");
  });

  it("escapes embedded single quotes", () => {
    expect(launchCommand({ runtimeExecutable: "echo", runtimeArgs: ["it's"] })).toBe(
      "echo 'it'\\''s'",
    );
  });

  it("quotes an empty arg rather than dropping it", () => {
    expect(launchCommand({ runtimeExecutable: "run", runtimeArgs: [""] })).toBe("run ''");
  });
});

describe("launchAction", () => {
  it("offers launch when nothing runs", () => {
    expect(launchAction(cfg({}))).toBe("launch");
  });

  it("offers focus for a pane we launched, even if the server inside died", () => {
    expect(launchAction(cfg({ sessionId: "s1", portListening: true }))).toBe("focus");
    expect(launchAction(cfg({ sessionId: "s1", portListening: false }))).toBe("focus");
  });

  it("never offers a second launch into a port something else holds", () => {
    expect(launchAction(cfg({ portListening: true }))).toBe("external");
  });

  it("offers launch for a portless config that has no pane", () => {
    expect(launchAction(cfg({ port: null }))).toBe("launch");
  });
});
