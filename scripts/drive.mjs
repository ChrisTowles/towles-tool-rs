#!/usr/bin/env node
// Live-drive the persistent app window opened by `npm run dev:drive`.
//
// Talks directly to the in-app W3C WebDriver server that tauri-plugin-wdio-webdriver
// runs inside the app on wdPort — plain fetch, NO @wdio/* import at runtime. Reads
// and IPC go through the session-less `POST /wdio/eval`; screenshots/clicks/nav use
// a short-lived W3C session that is deleted immediately so nothing leaks.
//
// Usage:
//   node scripts/drive.mjs status
//   node scripts/drive.mjs eval "document.title"
//   node scripts/drive.mjs invoke settings_get
//   node scripts/drive.mjs invoke journal_log '{"text":"hi"}'
//   node scripts/drive.mjs shot cockpit
//   node scripts/drive.mjs click "[data-screen=board]"
//   node scripts/drive.mjs clicktext "Board"
//   node scripts/drive.mjs type "input[name=q]" "hello"
//   node scripts/drive.mjs url /
//
// `click` dispatches a full pointerdown/mousedown/focus/pointerup/mouseup/
// click sequence (see `dispatchClick`) rather than the native W3C
// `element/click` endpoint — the native endpoint doesn't reliably open Radix
// `DropdownMenu`/`Popover` triggers (#35); the full event sequence does, in
// the same one-shot per-command session used everywhere else.
//
// shot/click/type/url each open-and-close their own short-lived WebDriver
// session by default; pass `--session <id>` (from `session-open`) to hold
// one open across several related actions instead:
//   node scripts/drive.mjs session-open              # prints a session id
//   node scripts/drive.mjs click "button" --session <id>
//   node scripts/drive.mjs click "[role=menuitem]" --session <id>
//   node scripts/drive.mjs session-close <id>
//
// Ports come from the rendered `.env`/`.env.local` (same as dev:drive):
// wdPort = the .env claim TT_E2E_WEBDRIVER_PORT, else TT_DEV_PORT + 3000.
import { writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { requireDevPort, resolveWebdriverPort } from "./slot-port.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";

const devPort = requireDevPort(repoRoot, { tag: "drive" });
const wdPort = resolveWebdriverPort(devPort);
const base = `http://127.0.0.1:${wdPort}`;

function fail(msg) {
  console.error(`[drive] ${msg}`);
  process.exit(1);
}

async function http(method, pathname, body) {
  let res;
  try {
    res = await fetch(base + pathname, {
      method,
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    fail(
      `can't reach the automation server at ${base} (${e.cause?.code || e.message}).\n` +
        `Is \`npm run dev:drive\` running in this slot?`,
    );
  }
  const text = await res.text();
  let json;
  try {
    json = text ? JSON.parse(text) : {};
  } catch {
    json = { raw: text };
  }
  return { res, json };
}

// --- session-less eval (reads + IPC) ---------------------------------------
// The Linux executor runs `(function(){ <script> }).apply(null, [...args, __done])`,
// so the script uses the last argument as the W3C async done-callback and reports
// back the {ok, value, undef} shape the /wdio/eval handler expects.
function wrapExpr(expr) {
  return `var __cb = arguments[arguments.length - 1];
(async () => {
  try {
    const __r = await (${expr});
    __cb({ ok: true, value: __r === undefined ? null : __r, undef: __r === undefined });
  } catch (e) {
    __cb({ ok: false, error: (e && e.message) || String(e) });
  }
})();`;
}

async function evalExpr(expr) {
  const { res, json } = await http("POST", "/wdio/eval", { script: wrapExpr(expr) });
  if (!res.ok || json.error) fail(`eval failed: ${json.error || `HTTP ${res.status}`}`);
  return json.undef ? undefined : json.value;
}

// --- W3C session (screenshots, clicks, nav) ---------------------------------
// By default each call opens a fresh session and tears it down immediately
// (`create`); pass `session: <id>` (from `session-open`) to run against an
// already-open, caller-managed session instead — see the `--session` flag.
async function createSession() {
  const created = await http("POST", "/session", { capabilities: { alwaysMatch: {} } });
  const sessionId = created.json?.value?.sessionId;
  if (!created.res.ok || !sessionId) {
    fail(`could not create a WebDriver session: ${JSON.stringify(created.json)}`);
  }
  return sessionId;
}

async function withSession(fn, existingSessionId) {
  if (existingSessionId) return fn(existingSessionId);
  const sessionId = await createSession();
  try {
    return await fn(sessionId);
  } finally {
    await http("DELETE", `/session/${sessionId}`).catch(() => {});
  }
}

/** Pull a trailing `--session <id>` flag out of a verb's args, wherever it
 * appears, so `--session` can be appended to any of `shot`/`click`/`type`/
 * `url` without disturbing their existing positional arguments. */
function extractSessionFlag(args) {
  const idx = args.indexOf("--session");
  if (idx === -1) return { session: null, rest: args };
  const session = args[idx + 1];
  if (!session) fail(`--session requires a session id (from \`session-open\`)`);
  return { session, rest: [...args.slice(0, idx), ...args.slice(idx + 2)] };
}

/** Dispatch a full pointerdown → mousedown → focus → pointerup → mouseup →
 * click sequence at `elId` inside the browsing context, via
 * `POST /session/{id}/execute/sync` rather than the native W3C
 * `POST /session/{id}/element/{id}/click` endpoint. The native click endpoint
 * does not reliably open Radix `DropdownMenu`/`Popover` triggers here — it
 * fires *something* the trigger doesn't react to, so `DismissableLayer`
 * never flips `data-state` to `open` (#35). The full event sequence does,
 * confirmed live: same one-shot session lifecycle either way, so this isn't
 * about session reuse across commands — just which endpoint synthesizes the
 * click. */
async function dispatchClick(sessionId, elId) {
  const script = `
    const el = arguments[0];
    const opts = { bubbles: true, cancelable: true, composed: true, pointerId: 1, isPrimary: true, button: 0 };
    el.dispatchEvent(new PointerEvent("pointerdown", opts));
    el.dispatchEvent(new MouseEvent("mousedown", opts));
    el.focus();
    el.dispatchEvent(new PointerEvent("pointerup", opts));
    el.dispatchEvent(new MouseEvent("mouseup", opts));
    el.dispatchEvent(new MouseEvent("click", opts));
  `;
  const { res, json } = await http("POST", `/session/${sessionId}/execute/sync`, {
    script,
    args: [{ [ELEMENT_KEY]: elId }],
  });
  if (!res.ok) fail(`click dispatch failed: ${JSON.stringify(json)}`);
}

async function findElement(sessionId, selector) {
  const { res, json } = await http("POST", `/session/${sessionId}/element`, {
    using: "css selector",
    value: selector,
  });
  const elId = json?.value?.[ELEMENT_KEY];
  if (!res.ok || !elId) fail(`no element matched \`${selector}\``);
  return elId;
}

function fmt(v) {
  if (v === undefined) return "(undefined)";
  if (typeof v === "string") return v;
  return JSON.stringify(v, null, 2);
}

// --- console errors ---------------------------------------------------------
// The app buffers console.error/warn + uncaught exceptions on `window` under
// VITE_WDIO (apps/client/src/lib/wdio-console.ts — keep this key in sync; it's
// a cross-process contract and a rename just makes the check go quiet).
// Without this, React's runtime complaints only reach the `dev:drive`
// terminal's stdout, a different process from this script.
const CONSOLE_KEY = "__ttConsoleErrors";

/** Read the buffer. `summary` returns just a count + the last few errors —
 * the buffer holds up to 200 × 2KB entries, too much to ship for a warning. */
async function readConsole({ clear = false, summary = false } = {}) {
  return await evalExpr(`(() => {
    const b = window[${JSON.stringify(CONSOLE_KEY)}];
    if (!b) return null;
    const errors = b.filter((e) => e.kind !== "warn");
    const out = ${summary}
      ? { count: errors.length, last: errors.slice(-3) }
      : b.slice();
    if (${clear}) b.length = 0;
    return out;
  })()`);
}

/** Warn if the page has logged errors. Runs after every verb, so a broken
 * render is impossible to miss even when the verb itself succeeded. */
async function surfaceConsoleErrors() {
  let found;
  try {
    found = await readConsole({ summary: true });
  } catch {
    return; // never let the check itself break a working command
  }
  // `null` = collector absent (not a VITE_WDIO build); stay quiet.
  if (!found || found.count === 0) return;
  console.error(
    `\n[drive] ⚠ ${found.count} console error(s) in the page — run \`drive.mjs console\` for detail:`,
  );
  for (const e of found.last) {
    console.error(`  [${e.kind}] ${e.text.split("\n")[0].slice(0, 160)}`);
  }
}

function usage(exitCode) {
  console.log(
    [
      "Live-drive the window opened by `npm run dev:drive`.",
      "",
      "  status                     is the automation server up?",
      '  eval "<js expression>"     run JS in the live window, print the result',
      "  invoke <cmd> [jsonArgs]    call a real Rust IPC command",
      "  shot <name> [--session id]     screenshot → e2e/screenshots/<name>.png",
      '  click "<css selector>" [--session id]   click an element in the shared window',
      '  clicktext "<text>"         click a button/link by its visible text',
      '  type "<css selector>" <text> [--session id]   type into an element',
      "  url <path> [--session id]  navigate the window",
      "  session-open               open a session that outlives one command, print its id",
      "  session-close <id>         close a session opened with session-open",
      "  console [--clear]          console errors/warnings the page has logged",
      "",
      "Every verb also prints a ⚠ summary when the page has logged errors —",
      "React reports invalid markup at runtime, and nothing else here sees it.",
    ].join("\n"),
  );
  process.exit(exitCode);
}

// --- verbs -----------------------------------------------------------------
const [verb, ...rest] = process.argv.slice(2);

switch (verb) {
  case "status": {
    const { json } = await http("GET", "/status");
    const payload = json.value ?? json;
    console.log(fmt(payload));
    process.exit(payload?.ready ? 0 : 1);
    break;
  }
  case "eval": {
    const expr = rest.join(" ");
    if (!expr) fail(`usage: drive.mjs eval "<js expression>"`);
    console.log(fmt(await evalExpr(expr)));
    break;
  }
  case "invoke": {
    const [cmd, argsJson] = rest;
    if (!cmd) fail(`usage: drive.mjs invoke <command> [jsonArgs]`);
    const args = argsJson ?? "{}";
    try {
      JSON.parse(args);
    } catch {
      fail(`invalid JSON args: ${argsJson}`);
    }
    const expr = `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${args})`;
    console.log(fmt(await evalExpr(expr)));
    break;
  }
  case "session-open": {
    const sessionId = await createSession();
    console.log(sessionId);
    break;
  }
  case "session-close": {
    const sessionId = rest[0];
    if (!sessionId) fail(`usage: drive.mjs session-close <id>`);
    const { res, json } = await http("DELETE", `/session/${sessionId}`);
    if (!res.ok) fail(`session-close failed: ${JSON.stringify(json)}`);
    console.log(`closed session ${sessionId}`);
    break;
  }
  case "shot": {
    const { session, rest: args } = extractSessionFlag(rest);
    const name = (args[0] || "shot").replace(/[^\w.-]/g, "_");
    const dir = path.join(repoRoot, "e2e/screenshots");
    await mkdir(dir, { recursive: true });
    const file = path.join(dir, `${name}.png`);
    const b64 = await withSession(async (s) => {
      const { res, json } = await http("GET", `/session/${s}/screenshot`);
      if (!res.ok || !json.value) fail(`screenshot failed: ${JSON.stringify(json)}`);
      return json.value;
    }, session);
    await writeFile(file, Buffer.from(b64, "base64"));
    console.log(file);
    break;
  }
  case "click": {
    const { session, rest: args } = extractSessionFlag(rest);
    const sel = args.join(" ");
    if (!sel) fail(`usage: drive.mjs click "<css selector>" [--session id]`);
    await withSession(async (s) => {
      const el = await findElement(s, sel);
      await dispatchClick(s, el);
    }, session);
    console.log(`clicked ${sel}`);
    break;
  }
  case "clicktext": {
    const text = rest.join(" ").trim();
    if (!text) fail(`usage: drive.mjs clicktext "<visible text>"`);
    // Runs in the live window via the session-less eval path (no CSS selector
    // needed): find every clickable element, match trimmed innerText/value,
    // and dispatch a real click. Returns a structured result so ambiguous or
    // missing text can report the candidate texts we actually found.
    const result = await evalExpr(`(() => {
      const target = ${JSON.stringify(text)};
      const sel = 'button, a, [role=button], [role=link], [role=menuitem],' +
        ' [role=menuitemradio], [role=tab], [role=option], summary,' +
        ' input[type=button], input[type=submit], input[type=reset]';
      const nodes = Array.from(document.querySelectorAll(sel));
      const label = (n) => ((n.innerText ?? n.value ?? n.textContent ?? '') + '').trim();
      const matches = nodes.filter((n) => label(n) === target);
      if (matches.length === 1) {
        matches[0].click();
        return { clicked: true };
      }
      const candidates = [...new Set(nodes.map(label).filter(Boolean))].sort();
      if (matches.length === 0) return { clicked: false, reason: 'none', candidates };
      return { clicked: false, reason: 'ambiguous', count: matches.length, candidates };
    })()`);
    if (!result?.clicked) {
      const list = (result?.candidates ?? []).map((c) => `  - ${c}`).join("\n");
      if (result?.reason === "ambiguous") {
        fail(
          `\`${text}\` matched ${result.count} clickable elements (ambiguous).\n` +
            `Clickable texts found:\n${list}`,
        );
      }
      fail(
        `no clickable element with visible text \`${text}\`.\n` +
          `Clickable texts found:\n${list || "  (none)"}`,
      );
    }
    console.log(`clicked "${text}"`);
    break;
  }
  case "type": {
    const { session, rest: args } = extractSessionFlag(rest);
    const sel = args[0];
    const text = args.slice(1).join(" ");
    if (!sel || args.length < 2) {
      fail(`usage: drive.mjs type "<css selector>" <text> [--session id]`);
    }
    await withSession(async (s) => {
      const el = await findElement(s, sel);
      const { res, json } = await http("POST", `/session/${s}/element/${el}/value`, { text });
      if (!res.ok) fail(`type failed: ${JSON.stringify(json)}`);
    }, session);
    console.log(`typed into ${sel}`);
    break;
  }
  case "url": {
    const { session, rest: args } = extractSessionFlag(rest);
    const p = args[0] || "/";
    const full = `http://localhost:${devPort}${p.startsWith("/") ? p : `/${p}`}`;
    await withSession(async (s) => {
      const { res, json } = await http("POST", `/session/${s}/url`, { url: full });
      if (!res.ok) fail(`navigate failed: ${JSON.stringify(json)}`);
    }, session);
    console.log(`navigated to ${full}`);
    break;
  }
  case "console": {
    const entries = await readConsole({ clear: rest.includes("--clear") });
    if (entries === null) {
      fail(
        "no console collector in the page — is this a VITE_WDIO build (`npm run dev:drive`)?",
      );
    }
    if (entries.length === 0) {
      console.log("(no console errors or warnings)");
      break;
    }
    for (const e of entries) {
      console.log(`[${e.kind}] ${new Date(e.at).toISOString().slice(11, 19)} ${e.text}`);
    }
    // Non-zero on real errors so a caller can gate on this verb.
    process.exit(entries.some((e) => e.kind !== "warn") ? 1 : 0);
    break;
  }
  case undefined:
    usage(0);
    break;
  default:
    console.error(`[drive] unknown verb: ${verb}\n`);
    usage(1);
}

// Ran a verb successfully — say so if the page is nonetheless broken.
if (verb !== "console" && verb !== "status") {
  await surfaceConsoleErrors();
}
