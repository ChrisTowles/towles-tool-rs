#!/usr/bin/env bun
// YAGNI mode hook for the tt plugin — one script for both hook events.
//
// Concept adapted from the "ponytail" plugin by Dietrich Gebert (MIT),
// https://github.com/DietrichGebert/ponytail — specifically its flag-file
// mode tracking (hooks/ponytail-mode-tracker.js), SessionStart ruleset
// re-injection (hooks/ponytail-activate.js), and statusline badge.
// Simplified for tt: no env-var/config-file mode resolution and no
// per-level skill filtering — mode is off until an explicit /yagni, then
// persists across sessions via the flag file until turned off.

import { mkdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

export const MODES = ["lite", "full", "ultra"] as const;
export type Mode = (typeof MODES)[number];

export type PromptAction = { mode: Mode } | { off: true } | null;

export function defaultFlagPath(): string {
  return join(homedir(), ".claude", ".tt-yagni-mode");
}

export function readMode(flagPath: string): Mode | null {
  try {
    const raw = readFileSync(flagPath, "utf8").trim().toLowerCase();
    return (MODES as readonly string[]).includes(raw) ? (raw as Mode) : null;
  } catch {
    return null;
  }
}

export function setMode(flagPath: string, mode: Mode): void {
  mkdirSync(dirname(flagPath), { recursive: true });
  writeFileSync(flagPath, mode);
}

export function clearMode(flagPath: string): void {
  try {
    unlinkSync(flagPath);
  } catch {
    // already gone — fine
  }
}

// Parse a user prompt for /yagni commands and deactivation phrases.
export function parsePrompt(prompt: string): PromptAction {
  const p = prompt.trim().toLowerCase();
  if (/\b(stop yagni|normal mode)\b/.test(p)) return { off: true };
  // Anchored so /yagni-review (the one-shot review skill) never matches.
  const m = p.match(/^\/(?:tt:)?yagni(?:\s+(\w+))?\s*$/);
  if (!m) return null;
  const arg = m[1] ?? "full";
  if (arg === "off") return { off: true };
  return { mode: (MODES as readonly string[]).includes(arg) ? (arg as Mode) : "full" };
}

// SessionStart: if a mode is active, re-inject the yagni ruleset as context.
export function sessionStartContext(flagPath: string, skillPath: string): string {
  const mode = readMode(flagPath);
  if (!mode) return "";
  let body = "";
  try {
    body = readFileSync(skillPath, "utf8").replace(/^---[\s\S]*?---\s*/, "");
  } catch {
    // skill file missing — the header line alone still signals the mode
  }
  return `YAGNI MODE ACTIVE — level: ${mode}\n\n${body}`;
}

export function userPromptContext(flagPath: string, prompt: string): string {
  const action = parsePrompt(prompt);
  if (action === null) return "";
  if ("off" in action) {
    const wasActive = readMode(flagPath) !== null;
    clearMode(flagPath);
    return wasActive ? "YAGNI MODE OFF" : "";
  }
  setMode(flagPath, action.mode);
  return `YAGNI MODE ACTIVE — level: ${action.mode}`;
}

if (import.meta.main) {
  const flagPath = defaultFlagPath();
  let data: { hook_event_name?: string; prompt?: string } = {};
  try {
    // Strip UTF-8 BOM some shells prepend when piping (breaks JSON.parse).
    data = JSON.parse((await Bun.stdin.text()).replace(/^\uFEFF/, ""));
  } catch {
    process.exit(0);
  }
  if (data.hook_event_name === "SessionStart") {
    const skillPath = join(import.meta.dirname, "..", "skills", "yagni", "SKILL.md");
    process.stdout.write(sessionStartContext(flagPath, skillPath));
  } else if (data.hook_event_name === "UserPromptSubmit") {
    process.stdout.write(userPromptContext(flagPath, data.prompt ?? ""));
  }
}
