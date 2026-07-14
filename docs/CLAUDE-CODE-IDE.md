# Claude Code IDE integration

Towles Tool acts as an **IDE** for Claude Code sessions running in its embedded
terminals: highlighting lines in a diff pane feeds that file + line range to the
`claude` session in the same folder as selection context, exactly like
highlighting code in VS Code does. This documents the wire protocol (reverse
engineered) and how the app implements it.

> Protocol verified against the VS Code extension `anthropic.claude-code`
> 2.1.207 and Claude Code CLI 2.1.208 (2026-07). It is a private protocol —
> re-verify against the shipped extension when something breaks.

## The protocol

The model is inverted from what you might expect: **the IDE hosts a WebSocket
MCP server; the Claude Code CLI is the client** that dials in. The IDE
advertises itself with a lockfile; the CLI discovers it by env var or cwd.

### Discovery

- The IDE picks a free localhost port, starts a WebSocket server on
  `127.0.0.1:<port>`, and writes `~/.claude/ide/<PORT>.lock` (file mode 0600,
  dir 0700). **The port is the filename** — the CLI parses it from
  `basename.replace(".lock")`; there is no port field in the JSON.
- Lockfile JSON (camelCase):

  ```json
  {
    "pid": 1405950,
    "workspaceFolders": ["/abs/checkout/dir"],
    "ideName": "Towles Tool",
    "transport": "ws",
    "runningInWindows": false,
    "authToken": "<random uuid>"
  }
  ```

- The IDE exports `CLAUDE_CODE_SSE_PORT=<port>` into the shell it spawns.
  (`ENABLE_IDE_INTEGRATION` no longer exists in current CLI versions.)
- CLI-side matching: a lockfile is accepted when the port equals
  `CLAUDE_CODE_SSE_PORT` (this skips all other checks), **or** the CLI's cwd is
  at/under one of `workspaceFolders` *and* the lockfile `pid` is alive and
  related to the CLI process. Because towles-tool sets the env var per
  terminal, each `claude` deterministically pairs with the pane it runs in.
- The lockfile is deleted when the server shuts down. Stale files (crash) are
  ignored by the CLI's pid-liveness check.

### Connection

- Transport: JSON-RPC 2.0 over WebSocket, one JSON object per text frame.
- WebSocket subprotocol: `mcp` (the CLI requests it; the server must echo it).
- Auth: the CLI sends the header `x-claude-code-ide-authorization: <authToken>`
  on the upgrade request. Mismatch → close with code 1008.
- Handshake: standard MCP — `initialize`, `notifications/initialized`,
  `tools/list`, then `tools/call` as needed.
- **Serve connections concurrently.** Claude Code >= 2.1.x is multi-process:
  the interactive TUI *and* its session daemon (`claude daemon run`) each dial
  the IDE server, and the session that actually consumes selection context is
  daemon-run. A single-client server (VS Code's historical behavior) starves
  the daemon and selections never reach prompts. Broadcast every notification
  to all authenticated connections.
- The CLI may ask once per session ("`/ide` → Towles Tool", then
  "enable auto-connect?"); with auto-connect enabled it attaches on startup
  whenever `CLAUDE_CODE_SSE_PORT` matches. Sessions launched headless from the
  launcher screen ("background sessions") do not consume selection context —
  only foreground interactive sessions do.

### Notifications, IDE → CLI (no `id`)

`selection_changed` — the ambient "user is looking at this" signal. VS Code
sends it on every selection change, debounced 300 ms. Lines and characters are
**0-based**:

```json
{"jsonrpc":"2.0","method":"selection_changed","params":{
  "text":"<selected text or empty>",
  "filePath":"/abs/file.rs",
  "fileUrl":"file:///abs/file.rs",
  "selection":{"start":{"line":10,"character":0},
               "end":{"line":12,"character":0},"isEmpty":false}}}
```

The CLI caches the latest one and attaches it to the next prompt (the
"user selected lines X–Y of file Z" context you see in transcripts).

`at_mentioned` — the explicit "send this to Claude" gesture. Becomes an
`@file#Lx-y` reference in the prompt. `lineStart`/`lineEnd` are 0-based and
omitted when there is no selection:

```json
{"jsonrpc":"2.0","method":"at_mentioned","params":{
  "filePath":"/abs/file.rs","lineStart":10,"lineEnd":12}}
```

`diagnostics_changed` — `{"params":{"uris":["file:///..."]}}`; only signals
staleness. Diagnostics themselves are pulled via the `getDiagnostics` tool.

### Tools, CLI → IDE (`tools/call`)

All results use the MCP text-content envelope
`{"content":[{"type":"text","text":"<usually JSON>"}]}`. Tools not advertised
in `tools/list` are simply never called — the CLI degrades gracefully (e.g. no
`openDiff` → terminal diffs). The full VS Code set, for reference:

| Tool | Input | Notes |
| --- | --- | --- |
| `getCurrentSelection` | `{}` | `{success,text,filePath,fileUrl,selection}` of the active editor |
| `getLatestSelection` | `{}` | Last cached selection even if no longer active |
| `getWorkspaceFolders` | `{}` | `{folders:[{name,uri,path}]}` |
| `getOpenEditors` | `{}` | `{tabs:[{uri,isActive,label,…}]}` |
| `getDiagnostics` | `{uri?}` | `[{uri,linesInFile,diagnostics:[{message,severity,range,source,code}]}]`, 0-based |
| `openFile` | `{filePath,preview?,startText?,endText?,…}` | Focus a file, select a range |
| `openDiff` | `{old_file_path,new_file_path,new_file_contents,tab_name}` | Blocking accept/reject of a proposed edit |
| `close_tab` / `closeAllDiffTabs` | `{tab_name}` / `{}` | Diff-tab management |
| `checkDocumentDirty` / `saveDocument` | `{filePath}` | Editor dirty state |
| `executeCode` | `{code}` | Jupyter kernel (notebooks only) |

## Towles Tool's implementation

```
apps/client DiffPane gutter selection
        │  ide_set_selection / ide_at_mention (Tauri command, routed by folder dir)
        ▼
crates-tauri/tt-app/src/ide.rs      one IdeServer per embedded terminal:
        │                           127.0.0.1:0 listener, auth check, lockfile,
        │                           pushes notifications to the connected CLI
        ▼
crates/tt-ide                       Tauri-free protocol core: lockfile schema,
                                    JSON-RPC dispatcher, notification builders
```

- **One server per terminal.** `term_start` binds `127.0.0.1:0` (OS-assigned
  port — never hardcoded, per the multi-slot rule), writes
  `~/.claude/ide/<port>.lock` with `workspaceFolders = [terminal cwd]`, and
  stamps `CLAUDE_CODE_SSE_PORT` into that PTY's env. A `claude` started in the
  pane therefore pairs with exactly that pane — no cwd guessing across slots.
  The env stamp happens *after* `tt_exec::scrub_app_instance_env`, which
  deliberately strips any inherited `CLAUDE_CODE_SSE_PORT` (issue #39's nested
  session-identity scrub) — the scrub removes the outer world's value, then we
  stamp our own.
- **Lifecycle.** The server task and lockfile die with the session: explicit
  `term_kill`, replacement by a new `term_start` on the same id, and window
  teardown all drop the `IdeServer` handle, whose `Drop` removes the lockfile.
  Startup sweeps `~/.claude/ide` for lockfiles left by dead towles-tool
  processes.
- **Selection flow.** The diff pane's gutter selection calls
  `ide_set_selection` (debounced client-side, mirroring VS Code's 300 ms) with
  the folder dir, file path and **1-based** new-file line range; the command
  resolves absolute paths, converts to 0-based at the boundary, caches the
  selection per server (serving `getCurrentSelection`/`getLatestSelection`),
  and pushes `selection_changed` to every connected session rooted in that
  folder. "Send to Claude" fires `ide_at_mention` the same way.
- **Advertised tools**: `getCurrentSelection`, `getLatestSelection`,
  `getWorkspaceFolders`, `getOpenEditors`, `getDiagnostics` (real cargo/tsc
  results via the app's DiagHub — see `crates-tauri/tt-app/src/diagnostics.rs`),
  `checkDocumentDirty`, `openFile`, `openDiff`, `close_tab`,
  `closeAllDiffTabs`. Tools with app-side effects (`openFile`, `openDiff` and
  the close pair) are intercepted in the app shell before the pure dispatcher:
  `openFile` focuses the Files tab (with `startText`/`endText` anchor
  selection in Monaco); `openDiff` blocks the CLI's tool call on an in-app
  accept/reject review (Monaco DiffEditor; accept atomically writes the —
  possibly user-tweaked — proposed contents and answers `FILE_SAVED` +
  contents, reject answers `DIFF_REJECTED` + tab name). Not implemented:
  `executeCode` (notebooks), `saveDocument` (the viewer autosurfaces dirty
  state instead).
- **Status surface.** Connect/disconnect emits `ide://status`
  (`{termId, connected}`); the diff pane shows a "Claude connected" badge so
  you know a highlight is actually going somewhere.

### Future work

- Full LSP (rust-analyzer / typescript-language-server bridged to Monaco via
  monaco-languageclient) — deliberately deferred: it costs real memory per
  slot, and the cargo/tsc diagnostics above may already cover the need.
  Revisit as a per-folder opt-in if they prove insufficient.
- `saveDocument` (needs nothing new — the blocking-tool machinery from
  `openDiff` can drive a save request into the viewer).
