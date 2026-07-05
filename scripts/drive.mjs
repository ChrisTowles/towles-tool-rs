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
//   node scripts/drive.mjs type "input[name=q]" "hello"
//   node scripts/drive.mjs url /
//
// Ports come from `.env.local` (same as dev:drive): wdPort = TT_DEV_PORT + 3000,
// override with TT_E2E_WEBDRIVER_PORT.
import { writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { resolveDevPort, resolveWebdriverPort } from "./slot-port.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";

const devPort = resolveDevPort(repoRoot);
if (!devPort) {
  fail(`TT_DEV_PORT=${process.env.TT_DEV_PORT} is not a valid port`);
}
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

// --- short-lived W3C session (screenshots, clicks, nav) --------------------
async function withSession(fn) {
  const created = await http("POST", "/session", { capabilities: { alwaysMatch: {} } });
  const sessionId = created.json?.value?.sessionId;
  if (!created.res.ok || !sessionId) {
    fail(`could not create a WebDriver session: ${JSON.stringify(created.json)}`);
  }
  try {
    return await fn(sessionId);
  } finally {
    await http("DELETE", `/session/${sessionId}`).catch(() => {});
  }
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

function usage(exitCode) {
  console.log(
    [
      "Live-drive the window opened by `npm run dev:drive`.",
      "",
      "  status                     is the automation server up?",
      '  eval "<js expression>"     run JS in the live window, print the result',
      "  invoke <cmd> [jsonArgs]    call a real Rust IPC command",
      "  shot <name>                screenshot → e2e/screenshots/<name>.png",
      '  click "<css selector>"     click an element in the shared window',
      '  type "<css selector>" <text>   type into an element',
      "  url <path>                 navigate the window",
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
  case "shot": {
    const name = (rest[0] || "shot").replace(/[^\w.-]/g, "_");
    const dir = path.join(repoRoot, "e2e/screenshots");
    await mkdir(dir, { recursive: true });
    const file = path.join(dir, `${name}.png`);
    const b64 = await withSession(async (s) => {
      const { res, json } = await http("GET", `/session/${s}/screenshot`);
      if (!res.ok || !json.value) fail(`screenshot failed: ${JSON.stringify(json)}`);
      return json.value;
    });
    await writeFile(file, Buffer.from(b64, "base64"));
    console.log(file);
    break;
  }
  case "click": {
    const sel = rest.join(" ");
    if (!sel) fail(`usage: drive.mjs click "<css selector>"`);
    await withSession(async (s) => {
      const el = await findElement(s, sel);
      const { res, json } = await http("POST", `/session/${s}/element/${el}/click`, {});
      if (!res.ok) fail(`click failed: ${JSON.stringify(json)}`);
    });
    console.log(`clicked ${sel}`);
    break;
  }
  case "type": {
    const sel = rest[0];
    const text = rest.slice(1).join(" ");
    if (!sel || rest.length < 2) fail(`usage: drive.mjs type "<css selector>" <text>`);
    await withSession(async (s) => {
      const el = await findElement(s, sel);
      const { res, json } = await http("POST", `/session/${s}/element/${el}/value`, { text });
      if (!res.ok) fail(`type failed: ${JSON.stringify(json)}`);
    });
    console.log(`typed into ${sel}`);
    break;
  }
  case "url": {
    const p = rest[0] || "/";
    const full = `http://localhost:${devPort}${p.startsWith("/") ? p : `/${p}`}`;
    await withSession(async (s) => {
      const { res, json } = await http("POST", `/session/${s}/url`, { url: full });
      if (!res.ok) fail(`navigate failed: ${JSON.stringify(json)}`);
    });
    console.log(`navigated to ${full}`);
    break;
  }
  case undefined:
    usage(0);
    break;
  default:
    console.error(`[drive] unknown verb: ${verb}\n`);
    usage(1);
}
