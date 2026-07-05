//! Claude Code agent watcher — hybrid edition (phase T7 of
//! docs/AGENTBOARD-PORT.md; rewrites the journal-first port of slot-1
//! `runtime/agents/watchers/claude-code.ts`).
//!
//! **Discovery, liveness, and status come from `claude agents --all --json`**
//! ([`crate::claude_cli`]) — the supported CLI surface — instead of scanning
//! `~/.claude/projects/**/*.jsonl` and inferring status from message roles.
//! **Journals are enrichment only**: incremental tail reads supply what the
//! CLI doesn't expose (model, last tool, token usage → cache countdown,
//! sub-agents, `/loop` wakeups, and the first-prompt thread name).
//!
//! Per scan:
//! 1. list live agents from the CLI;
//! 2. resolve each to a session — `resolve_session_by_pid` first (the tmux
//!    server walks the pid's ancestry to a pane), then the cwd;
//! 3. status: `busy`/`waiting` pass straight through (the vocabulary now
//!    follows the CLI); `idle` takes the journal's view when it is
//!    complete/waiting (preserving the unseen-✓ flow), else stays idle;
//! 4. enrich from the journal tail (offset at the last newline boundary —
//!    adopted fix #1 — with shrink-reset);
//! 5. sessions that disappeared from the CLI get one final journal read and a
//!    terminal emit: done if the journal completed, interrupted if it still
//!    looked mid-run.
//!
//! What this drops vs. the journal-first watcher (deliberate): sessions that
//! exited before the server started never appear (the CLI only lists live
//! processes), and the `~/.claude/sessions/<pid>.json` liveness files are no
//! longer read at all.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Deserialize;

use crate::claude_cli::CliAgent;
use crate::types::{AgentEvent, AgentEventDetails, AgentStatus, LoopInfo, SubagentInfo};
use crate::watcher::{AgentWatcher, JSONL_SUFFIX, WatcherContext};
use crate::watchers::claude_usage::{ClaudeUsageSummary, RawUsage, extract_usage_summary};

const NAME: &str = "claude-code";
/// The shared CLI snapshot TTL (watcher 2s tick, pane scan 3s, engine rebuilds).
pub const CLI_CACHE_TTL_MS: u64 = 1500;

// --- Tolerant journal-entry types (unchanged from the journal-first port) ---

/// One parsed journal line. Tolerant: every field optional, unknown fields ignored.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawEntry {
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub message: Option<RawMessage>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawMessage {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub content: Option<RawContent>,
    #[serde(default)]
    pub usage: Option<RawUsage>,
}

/// Message content: a bare string or an array of content blocks.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RawContent {
    Text(String),
    Items(Vec<RawContentItem>),
}

impl RawContent {
    fn items(&self) -> &[RawContentItem] {
        match self {
            RawContent::Items(v) => v,
            RawContent::Text(_) => &[],
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawContentItem {
    #[serde(rename = "type", default)]
    pub item_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<RawToolInput>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawToolInput {
    #[serde(rename = "delaySeconds", default)]
    pub delay_seconds: Option<f64>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Parse an ISO-8601 / RFC3339 timestamp to epoch ms (JS `Date.parse` equivalent).
pub fn parse_timestamp_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp_millis())
}

// --- Pure journal derivation (§3 status table, kept for enrichment) ---

/// Derive a status from one entry, or `None` (ignored) — the §3 decision table.
/// With CLI-driven liveness this only informs the `idle` refinement and the
/// exit-time terminal emit.
pub fn determine_status(entry: &RawEntry) -> Option<AgentStatus> {
    let msg = entry.message.as_ref()?;
    let role = msg.role.as_deref().filter(|r| !r.is_empty())?;
    let items = msg.content.as_ref().map(RawContent::items).unwrap_or(&[]);

    match role {
        "assistant" => {
            let tool_uses: Vec<&RawContentItem> =
                items.iter().filter(|c| c.item_type.as_deref() == Some("tool_use")).collect();
            if tool_uses.is_empty() {
                return Some(AgentStatus::Complete);
            }
            let all_asking = tool_uses.iter().all(|c| c.name.as_deref() == Some("AskUserQuestion"));
            Some(if all_asking { AgentStatus::Waiting } else { AgentStatus::Busy })
        }
        "user" => Some(AgentStatus::Busy),
        _ => None,
    }
}

/// The thread name from the first qualifying user message (skips system-like
/// `<…>`/`{…}` first lines). Ports `extractThreadName`.
pub fn extract_thread_name(entry: &RawEntry) -> Option<String> {
    let msg = entry.message.as_ref()?;
    if msg.role.as_deref() != Some("user") {
        return None;
    }
    let text = match msg.content.as_ref()? {
        RawContent::Text(s) => Some(s.as_str()),
        RawContent::Items(items) => items
            .iter()
            .find(|c| {
                c.item_type.as_deref() == Some("text")
                    && c.text.as_deref().is_some_and(|t| !t.is_empty())
            })
            .and_then(|c| c.text.as_deref()),
    }?;
    if text.is_empty() || text.starts_with('<') || text.starts_with('{') {
        return None;
    }
    Some(text.chars().take(80).collect())
}

/// Derive `(thread_name, status)` for a live agent detected outside the CLI
/// snapshot (via `procenv::scan_session_agents`) by reading its transcript.
/// Bounded reads keep large transcripts cheap: the thread name lives near the
/// top (first user message), the latest status near the bottom (last message).
/// Falls back to `Idle` when no status line parses.
pub fn enrich_from_transcript(path: &Path) -> (Option<String>, AgentStatus) {
    const WINDOW: u64 = 128 * 1024;
    let head = read_window(path, 0, WINDOW);
    let thread_name = parse_journal_lines(&head).iter().find_map(extract_thread_name);
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let tail = read_window(path, len.saturating_sub(WINDOW), WINDOW);
    let status = parse_journal_lines(&tail)
        .iter()
        .rev()
        .find_map(determine_status)
        .unwrap_or(AgentStatus::Idle);
    (thread_name, status)
}

/// Read up to `max` bytes of `path` from byte offset `start`, lossily decoded.
/// Partial JSONL lines at either edge simply fail to parse and are dropped by
/// `parse_journal_lines`, so no newline alignment is needed.
fn read_window(path: &Path, start: u64, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if f.take(max).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// The most recent non-AskUserQuestion tool name. Ports `extractLastTool`.
pub fn extract_last_tool(entries: &[RawEntry]) -> Option<String> {
    for entry in entries.iter().rev() {
        let Some(msg) = &entry.message else { continue };
        if msg.role.as_deref() != Some("assistant") {
            continue;
        }
        let Some(RawContent::Items(items)) = &msg.content else {
            continue;
        };
        for item in items {
            if item.item_type.as_deref() != Some("tool_use") {
                continue;
            }
            let Some(name) = &item.name else { continue };
            if name == "AskUserQuestion" {
                continue;
            }
            return Some(name.clone());
        }
    }
    None
}

/// `/loop` state from the most recent `ScheduleWakeup` tool call. Ports `extractLoopState`.
pub fn extract_loop_state(entries: &[RawEntry]) -> Option<LoopInfo> {
    for entry in entries.iter().rev() {
        let Some(msg) = &entry.message else { continue };
        if msg.role.as_deref() != Some("assistant") {
            continue;
        }
        let Some(RawContent::Items(items)) = &msg.content else {
            continue;
        };
        for item in items {
            if item.item_type.as_deref() != Some("tool_use")
                || item.name.as_deref() != Some("ScheduleWakeup")
            {
                continue;
            }
            let delay = item.input.as_ref().and_then(|i| i.delay_seconds);
            let scheduled_at = entry.timestamp.as_deref().and_then(parse_timestamp_ms);
            let (Some(delay), Some(ts)) = (delay, scheduled_at) else {
                return None;
            };
            let reason = item.input.as_ref().and_then(|i| i.reason.clone());
            return Some(LoopInfo { next_wake_at: ts + (delay * 1000.0) as i64, reason });
        }
    }
    None
}

/// Order-independent change signature for a set of sub-agents. Ports `subagentSignature`.
pub fn subagent_signature(subagents: &[SubagentInfo]) -> String {
    let mut sigs: Vec<String> = subagents
        .iter()
        .map(|s| {
            format!(
                "{} {}",
                s.agent_type.as_deref().unwrap_or(""),
                s.description.as_deref().unwrap_or("")
            )
        })
        .collect();
    sigs.sort();
    sigs.join("")
}

#[derive(Deserialize)]
struct SubagentMeta {
    #[serde(rename = "agentType", default)]
    agent_type: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Active sub-agents in `<session>/subagents/` — those modified within the 2-min
/// window, most-recent first. Ports `readActiveSubagents`.
pub fn read_active_subagents(subagents_dir: &Path, now_ms: i64) -> Vec<SubagentInfo> {
    let Ok(entries) = std::fs::read_dir(subagents_dir) else {
        return Vec::new();
    };
    let mut active: Vec<(SubagentInfo, i64)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("agent-") || !name.ends_with(JSONL_SUFFIX) {
            continue;
        }
        let path = entry.path();
        let Some(mtime) = mtime_ms(&path) else {
            continue;
        };
        if now_ms - mtime > crate::types::JOURNAL_IDLE_TIMEOUT_MS {
            continue;
        }
        // Sibling `agent-<id>.meta.json`; missing/unreadable meta still counts (as `{}`).
        let info = meta_path(&path)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<SubagentMeta>(&t).ok())
            .map(|m| SubagentInfo { agent_type: m.agent_type, description: m.description })
            .unwrap_or_default();
        active.push((info, mtime));
    }
    active.sort_by_key(|(_, m)| std::cmp::Reverse(*m));
    active.into_iter().map(|(info, _)| info).collect()
}

fn meta_path(jsonl_path: &Path) -> Option<PathBuf> {
    let s = jsonl_path.to_str()?;
    let base = s.strip_suffix(JSONL_SUFFIX)?;
    Some(PathBuf::from(format!("{base}.meta.json")))
}

/// Parse newline-delimited JSON, skipping blank/malformed lines. Ports `parseJournalLines`.
pub fn parse_journal_lines(text: &str) -> Vec<RawEntry> {
    text.split('\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<RawEntry>(l).ok())
        .collect()
}

/// Byte length up to and including the last newline (0 if none) — the offset the
/// next read resumes from (adopted fix #1: never consume a partial trailing line).
fn consumed_len(bytes: &[u8]) -> usize {
    match bytes.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => 0,
    }
}

fn mtime_ms(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

// --- Details assembly (§6, unchanged) ---

fn summary_to_details(s: &ClaudeUsageSummary) -> AgentEventDetails {
    AgentEventDetails {
        model: Some(s.model.clone()),
        context_used: Some(s.context_used),
        context_max: Some(s.context_max),
        cache_expires_at: s.cache_expires_at,
        cache_ttl_ms: s.cache_ttl_ms,
        last_activity_at: Some(s.last_activity_at),
        last_tool: None,
        subagents: None,
        r#loop: None,
    }
}

fn build_details(
    usage: Option<&ClaudeUsageSummary>,
    last_tool: Option<&str>,
    subagents: &[SubagentInfo],
    loop_state: Option<&LoopInfo>,
) -> Option<AgentEventDetails> {
    let has_subagents = !subagents.is_empty();
    if usage.is_none() && last_tool.is_none() && !has_subagents && loop_state.is_none() {
        return None;
    }
    let mut base = usage.map(summary_to_details).unwrap_or_default();
    if let Some(t) = last_tool {
        base.last_tool = Some(t.to_string());
    }
    if has_subagents {
        base.subagents = Some(subagents.to_vec());
    }
    if let Some(l) = loop_state {
        base.r#loop = Some(l.clone());
    }
    Some(base)
}

/// Refine a CLI `idle` into the journal's view when that view is one the UI
/// treats specially: a completed turn stays `complete` (unseen-✓ flow) and
/// an open question stays `waiting`; anything else is plain `idle`.
pub fn refine_idle(journal_status: AgentStatus) -> AgentStatus {
    match journal_status {
        AgentStatus::Complete => AgentStatus::Complete,
        AgentStatus::Waiting => AgentStatus::Waiting,
        _ => AgentStatus::Idle,
    }
}

/// Terminal status when a session disappears from the CLI list: `complete` if
/// its journal finished, `interrupted` if it still looked mid-run.
pub fn exit_status(journal_status: AgentStatus) -> AgentStatus {
    match journal_status {
        AgentStatus::Complete | AgentStatus::Waiting => AgentStatus::Complete,
        _ => AgentStatus::Interrupted,
    }
}

// --- Per-session journal enrichment state ---

#[derive(Debug, Clone)]
struct SessionState {
    /// Last status emitted for this session (the emit gate).
    emitted_status: Option<AgentStatus>,
    /// The journal's own status derivation (feeds `refine_idle`/`exit_status`).
    journal_status: AgentStatus,
    /// Next read offset = last-newline byte boundary (adopted fix #1).
    file_offset: u64,
    journal_path: Option<PathBuf>,
    thread_name: Option<String>,
    usage: Option<ClaudeUsageSummary>,
    last_tool: Option<String>,
    subagents: Vec<SubagentInfo>,
    subagent_sig: String,
    loop_state: Option<LoopInfo>,
    /// Signature of the last emitted details (usage/subagents/loop) —
    /// part of the emit gate (adopted fix #2: usage deltas broadcast
    /// without a status change).
    last_emit_sig: Option<String>,
    /// CLI fields carried for emits.
    session: Option<String>,
    cli_name: Option<String>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            emitted_status: None,
            journal_status: AgentStatus::Idle,
            file_offset: 0,
            journal_path: None,
            thread_name: None,
            usage: None,
            last_tool: None,
            subagents: Vec::new(),
            subagent_sig: String::new(),
            loop_state: None,
            last_emit_sig: None,
            session: None,
            cli_name: None,
        }
    }
}

impl SessionState {
    fn details(&self) -> Option<AgentEventDetails> {
        build_details(
            self.usage.as_ref(),
            self.last_tool.as_deref(),
            &self.subagents,
            self.loop_state.as_ref(),
        )
    }
}

// --- Watcher ---

/// The hybrid claude-code watcher: CLI discovery, journal enrichment.
pub struct ClaudeCodeAgentWatcher {
    projects_dir: PathBuf,
    sessions: HashMap<String, SessionState>,
    agents_source: Box<dyn Fn() -> Vec<CliAgent> + Send>,
}

impl ClaudeCodeAgentWatcher {
    /// Create rooted at `projects_dir` with an injectable CLI source (tests
    /// pass fixtures; production uses the cached real CLI).
    pub fn new(
        projects_dir: PathBuf,
        agents_source: Box<dyn Fn() -> Vec<CliAgent> + Send>,
    ) -> Self {
        Self { projects_dir, sessions: HashMap::new(), agents_source }
    }

    /// Create using the real `~/.claude/projects` + the shared cached CLI call.
    pub fn with_defaults() -> Self {
        let projects_dir =
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude").join("projects");
        Self::new(
            projects_dir,
            Box::new(|| {
                crate::claude_cli::fetch_agents_cached(std::time::Duration::from_millis(
                    CLI_CACHE_TTL_MS,
                ))
            }),
        )
    }

    /// Locate `<projects>/<encoded cwd>/<session id>.jsonl`. The naive
    /// `/`→`-` encoding guess covers most paths; dirs whose names contain
    /// dots/underscores (also encoded to `-` by Claude) fall back to probing
    /// every project dir for the session file.
    fn find_journal(&self, cwd: &str, session_id: &str) -> Option<PathBuf> {
        let file = format!("{session_id}{JSONL_SUFFIX}");
        let guess = self.projects_dir.join(cwd.replace('/', "-")).join(&file);
        if guess.exists() {
            return Some(guess);
        }
        let entries = std::fs::read_dir(&self.projects_dir).ok()?;
        for entry in entries.flatten() {
            let candidate = entry.path().join(&file);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    /// Incrementally read the session's journal tail into its state.
    fn enrich_from_journal(&mut self, session_id: &str, cwd: &str, now_ms: i64) {
        let path = match self.sessions.get(session_id).and_then(|s| s.journal_path.clone()) {
            Some(p) if p.exists() => Some(p),
            _ => self.find_journal(cwd, session_id),
        };
        let state = self.sessions.entry(session_id.to_string()).or_default();
        state.journal_path = path.clone();
        let Some(path) = path else { return };

        // Sub-agents live in a sibling dir; the parent journal can stay
        // static for minutes — compute every scan.
        if let Some(base) = path.to_str().and_then(|s| s.strip_suffix(JSONL_SUFFIX)) {
            let subagents =
                read_active_subagents(&PathBuf::from(format!("{base}/subagents")), now_ms);
            state.subagent_sig = subagent_signature(&subagents);
            state.subagents = subagents;
        }

        let Ok(meta) = std::fs::metadata(&path) else {
            return;
        };
        let size = meta.len();
        // Adopted fix #1: shrunk file → reset to 0 and re-derive from scratch
        // (all journal-derived state is stale, not just the offset).
        if size < state.file_offset {
            state.file_offset = 0;
            state.journal_status = AgentStatus::Idle;
            state.thread_name = None;
            state.usage = None;
            state.last_tool = None;
            state.loop_state = None;
        }
        if size == state.file_offset {
            return;
        }

        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let start = (state.file_offset as usize).min(bytes.len());
        let slice = &bytes[start..];
        let consumed = consumed_len(slice);
        if consumed == 0 {
            return;
        }
        let text = String::from_utf8_lossy(&slice[..consumed]);
        let parsed = parse_journal_lines(&text);
        state.file_offset += consumed as u64;

        for entry in &parsed {
            if state.thread_name.is_none()
                && let Some(name) = extract_thread_name(entry)
            {
                state.thread_name = Some(name);
            }
            if let Some(s) = determine_status(entry) {
                state.journal_status = s;
            }
        }
        if let Some(usage) = extract_usage_summary(&parsed) {
            state.usage = Some(usage);
        }
        if let Some(tool) = extract_last_tool(&parsed) {
            state.last_tool = Some(tool);
        }
        if let Some(loop_state) = extract_loop_state(&parsed) {
            state.loop_state = Some(loop_state);
        }
    }

    fn emit(
        ctx: &mut dyn WatcherContext,
        state: &SessionState,
        status: AgentStatus,
        session_id: &str,
        now_ms: i64,
    ) {
        let Some(session) = state.session.clone() else {
            return;
        };
        ctx.emit(AgentEvent {
            agent: NAME.to_string(),
            session,
            status,
            ts: now_ms,
            thread_id: Some(session_id.to_string()),
            // Journal first-prompt text beats the CLI's interactive slugs
            // (`proj-44`); background agents get descriptive CLI names.
            thread_name: state.thread_name.clone().or_else(|| state.cli_name.clone()),
            unseen: None,
            pane_id: None,
            details: state.details(),
        });
    }
}

impl AgentWatcher for ClaudeCodeAgentWatcher {
    fn name(&self) -> &str {
        NAME
    }

    fn scan(&mut self, ctx: &mut dyn WatcherContext, now_ms: i64) {
        let agents = (self.agents_source)();
        let live_ids: HashSet<String> = agents.iter().map(|a| a.session_id.clone()).collect();

        // Live sessions: resolve, enrich, emit on change.
        for agent in &agents {
            // pid → owning tmux pane's session first; cwd match as fallback.
            let session =
                ctx.resolve_session_by_pid(agent.pid).or_else(|| ctx.resolve_session(&agent.cwd));
            let Some(session) = session else {
                continue;
            };

            self.enrich_from_journal(&agent.session_id, &agent.cwd, now_ms);
            let state = self.sessions.get_mut(&agent.session_id).unwrap();
            state.session = Some(session);
            state.cli_name = agent.name.clone();

            let status = match agent.agent_status() {
                Some(AgentStatus::Idle) | None => refine_idle(state.journal_status),
                Some(s) => s,
            };

            // Emit gate: status change, or a details change (sub-agent set,
            // loop wake, usage — adopted fix #2) captured as a signature.
            let status_changed = state.emitted_status != Some(status);
            let sig = format!(
                "{}|{:?}|{:?}|{:?}",
                state.subagent_sig,
                state.loop_state.as_ref().map(|l| l.next_wake_at),
                state.usage.as_ref().map(|u| (u.context_used, u.cache_expires_at)),
                state.thread_name,
            );
            let details_changed = state.last_emit_sig.as_deref() != Some(sig.as_str());

            if status_changed || details_changed {
                state.emitted_status = Some(status);
                state.last_emit_sig = Some(sig);
                let state = state.clone();
                Self::emit(ctx, &state, status, &agent.session_id, now_ms);
            }
        }

        // Exited sessions: one final journal read, then a terminal emit.
        let gone: Vec<String> =
            self.sessions.keys().filter(|id| !live_ids.contains(*id)).cloned().collect();
        for session_id in gone {
            let cwd = String::new();
            self.enrich_from_journal(&session_id, &cwd, now_ms);
            let Some(state) = self.sessions.remove(&session_id) else {
                continue;
            };
            if state.session.is_none() || state.emitted_status.is_none() {
                // Never resolved/emitted — nothing on the board to finalize.
                continue;
            }
            let status = exit_status(state.journal_status);
            Self::emit(ctx, &state, status, &session_id, now_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    struct Ctx {
        by_dir: Vec<(String, String)>,
        by_pid: Vec<(i32, String)>,
        events: Vec<AgentEvent>,
    }

    impl Ctx {
        fn new() -> Self {
            Self { by_dir: Vec::new(), by_pid: Vec::new(), events: Vec::new() }
        }
    }

    impl WatcherContext for Ctx {
        fn resolve_session(&self, project_dir: &str) -> Option<String> {
            self.by_dir.iter().find(|(d, _)| d == project_dir).map(|(_, s)| s.clone())
        }
        fn resolve_session_by_pid(&self, pid: i32) -> Option<String> {
            self.by_pid.iter().find(|(p, _)| *p == pid).map(|(_, s)| s.clone())
        }
        fn emit(&mut self, event: AgentEvent) {
            self.events.push(event);
        }
    }

    fn cli_agent(pid: i32, cwd: &str, sid: &str, status: &str) -> CliAgent {
        CliAgent {
            pid,
            cwd: cwd.to_string(),
            kind: Some("interactive".into()),
            started_at: Some(1),
            session_id: sid.to_string(),
            name: Some(format!("slug-{pid}")),
            status: Some(status.to_string()),
            waiting_for: None,
        }
    }

    struct Fixture {
        _tmp: TempDir,
        projects: PathBuf,
        agents: Arc<Mutex<Vec<CliAgent>>>,
        watcher: ClaudeCodeAgentWatcher,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        let agents: Arc<Mutex<Vec<CliAgent>>> = Arc::new(Mutex::new(Vec::new()));
        let source = agents.clone();
        let watcher = ClaudeCodeAgentWatcher::new(
            projects.clone(),
            Box::new(move || source.lock().unwrap().clone()),
        );
        Fixture { _tmp: tmp, projects, agents, watcher }
    }

    fn write_journal(projects: &Path, cwd: &str, sid: &str, lines: &[&str]) -> PathBuf {
        let dir = projects.join(cwd.replace('/', "-"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{sid}.jsonl"));
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(&path, text).unwrap();
        path
    }

    const USER_LINE: &str = r#"{"timestamp":"2026-07-03T10:00:00.000Z","message":{"role":"user","content":"fix the flaky test"}}"#;
    const RUNNING_LINE: &str = r#"{"timestamp":"2026-07-03T10:00:05.000Z","message":{"role":"assistant","model":"claude-sonnet-5","content":[{"type":"tool_use","name":"Bash"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#;
    const DONE_LINE: &str = r#"{"timestamp":"2026-07-03T10:00:10.000Z","message":{"role":"assistant","content":[{"type":"text","text":"all done"}]}}"#;

    #[test]
    fn busy_agent_emits_running_with_journal_enrichment() {
        let mut f = fixture();
        write_journal(&f.projects, "/home/u/proj", "sid-1", &[USER_LINE, RUNNING_LINE]);
        *f.agents.lock().unwrap() = vec![cli_agent(100, "/home/u/proj", "sid-1", "busy")];
        let mut ctx = Ctx::new();
        ctx.by_dir.push(("/home/u/proj".into(), "proj".into()));

        f.watcher.scan(&mut ctx, 1_000);
        assert_eq!(ctx.events.len(), 1);
        let ev = &ctx.events[0];
        assert_eq!(ev.session, "proj");
        assert_eq!(ev.status, AgentStatus::Busy);
        assert_eq!(ev.thread_id.as_deref(), Some("sid-1"));
        // Journal first prompt beats the CLI slug.
        assert_eq!(ev.thread_name.as_deref(), Some("fix the flaky test"));
        let details = ev.details.as_ref().unwrap();
        assert_eq!(details.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(details.last_tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn enrich_from_transcript_reads_name_and_status() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("s.jsonl");

        // First user message → thread name; last assistant-complete → Complete.
        let mut text = [USER_LINE, DONE_LINE].join("\n");
        text.push('\n');
        std::fs::write(&path, &text).unwrap();
        let (name, status) = enrich_from_transcript(&path);
        assert_eq!(name.as_deref(), Some("fix the flaky test"));
        assert_eq!(status, AgentStatus::Complete);

        // Mid tool-run → Busy (task name still resolves from the head).
        let mut running = [USER_LINE, RUNNING_LINE].join("\n");
        running.push('\n');
        std::fs::write(&path, &running).unwrap();
        let (name2, status2) = enrich_from_transcript(&path);
        assert_eq!(name2.as_deref(), Some("fix the flaky test"));
        assert_eq!(status2, AgentStatus::Busy);

        // Missing file → no name, Idle fallback (never panics).
        let (n3, s3) = enrich_from_transcript(&tmp.path().join("missing.jsonl"));
        assert_eq!(n3, None);
        assert_eq!(s3, AgentStatus::Idle);
    }

    #[test]
    fn idle_refines_by_journal_and_waiting_maps_to_question() {
        let mut f = fixture();
        // Journal ends done → idle process shows Done (unseen-✓ flow).
        write_journal(&f.projects, "/home/u/a", "sid-done", &[USER_LINE, DONE_LINE]);
        // Journal mid-run → idle process shows Waiting.
        write_journal(&f.projects, "/home/u/b", "sid-mid", &[USER_LINE, RUNNING_LINE]);
        *f.agents.lock().unwrap() = vec![
            cli_agent(1, "/home/u/a", "sid-done", "idle"),
            cli_agent(2, "/home/u/b", "sid-mid", "idle"),
            cli_agent(3, "/home/u/a", "sid-perm", "waiting"),
        ];
        let mut ctx = Ctx::new();
        ctx.by_dir.push(("/home/u/a".into(), "a".into()));
        ctx.by_dir.push(("/home/u/b".into(), "b".into()));

        f.watcher.scan(&mut ctx, 1_000);
        let by_thread: std::collections::HashMap<&str, AgentStatus> =
            ctx.events.iter().map(|e| (e.thread_id.as_deref().unwrap(), e.status)).collect();
        assert_eq!(by_thread["sid-done"], AgentStatus::Complete);
        assert_eq!(by_thread["sid-mid"], AgentStatus::Idle);
        assert_eq!(by_thread["sid-perm"], AgentStatus::Waiting);
    }

    #[test]
    fn pid_resolution_beats_cwd() {
        let mut f = fixture();
        write_journal(&f.projects, "/home/u/proj", "sid-1", &[USER_LINE, RUNNING_LINE]);
        *f.agents.lock().unwrap() = vec![cli_agent(100, "/home/u/proj", "sid-1", "busy")];
        let mut ctx = Ctx::new();
        ctx.by_dir.push(("/home/u/proj".into(), "by-cwd".into()));
        ctx.by_pid.push((100, "by-pane".into()));

        f.watcher.scan(&mut ctx, 1_000);
        assert_eq!(ctx.events[0].session, "by-pane");
    }

    #[test]
    fn no_reemit_without_change_but_usage_delta_reemits() {
        let mut f = fixture();
        let path = write_journal(&f.projects, "/home/u/p", "sid-1", &[USER_LINE, RUNNING_LINE]);
        *f.agents.lock().unwrap() = vec![cli_agent(9, "/home/u/p", "sid-1", "busy")];
        let mut ctx = Ctx::new();
        ctx.by_dir.push(("/home/u/p".into(), "p".into()));

        f.watcher.scan(&mut ctx, 1_000);
        f.watcher.scan(&mut ctx, 3_000);
        assert_eq!(ctx.events.len(), 1, "steady state must not re-emit");

        // Usage growth without a status change re-emits (adopted fix #2).
        let more = r#"{"timestamp":"2026-07-03T10:00:20.000Z","message":{"role":"assistant","model":"claude-sonnet-5","content":[{"type":"tool_use","name":"Read"}],"usage":{"input_tokens":900,"output_tokens":50}}}"#;
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(more);
        text.push('\n');
        std::fs::write(&path, text).unwrap();

        f.watcher.scan(&mut ctx, 5_000);
        assert_eq!(ctx.events.len(), 2);
        assert_eq!(ctx.events[1].status, AgentStatus::Busy);
        assert_eq!(ctx.events[1].details.as_ref().unwrap().last_tool.as_deref(), Some("Read"));
    }

    #[test]
    fn exit_emits_done_or_interrupted_from_final_journal() {
        let mut f = fixture();
        let done_path =
            write_journal(&f.projects, "/home/u/a", "sid-done", &[USER_LINE, RUNNING_LINE]);
        write_journal(&f.projects, "/home/u/b", "sid-mid", &[USER_LINE, RUNNING_LINE]);
        *f.agents.lock().unwrap() = vec![
            cli_agent(1, "/home/u/a", "sid-done", "busy"),
            cli_agent(2, "/home/u/b", "sid-mid", "busy"),
        ];
        let mut ctx = Ctx::new();
        ctx.by_dir.push(("/home/u/a".into(), "a".into()));
        ctx.by_dir.push(("/home/u/b".into(), "b".into()));
        f.watcher.scan(&mut ctx, 1_000);
        ctx.events.clear();

        // sid-done's journal completes before it exits; sid-mid dies mid-run.
        let mut text = std::fs::read_to_string(&done_path).unwrap();
        text.push_str(DONE_LINE);
        text.push('\n');
        std::fs::write(&done_path, text).unwrap();
        f.agents.lock().unwrap().clear();

        f.watcher.scan(&mut ctx, 5_000);
        let by_thread: std::collections::HashMap<&str, AgentStatus> =
            ctx.events.iter().map(|e| (e.thread_id.as_deref().unwrap(), e.status)).collect();
        assert_eq!(by_thread["sid-done"], AgentStatus::Complete);
        assert_eq!(by_thread["sid-mid"], AgentStatus::Interrupted);
        // Gone for good: nothing further on later scans.
        ctx.events.clear();
        f.watcher.scan(&mut ctx, 7_000);
        assert!(ctx.events.is_empty());
    }

    #[test]
    fn unresolved_agents_never_emit_even_on_exit() {
        let mut f = fixture();
        write_journal(&f.projects, "/home/u/x", "sid-x", &[USER_LINE, RUNNING_LINE]);
        *f.agents.lock().unwrap() = vec![cli_agent(1, "/home/u/x", "sid-x", "busy")];
        let mut ctx = Ctx::new(); // resolves nothing
        f.watcher.scan(&mut ctx, 1_000);
        f.agents.lock().unwrap().clear();
        f.watcher.scan(&mut ctx, 3_000);
        assert!(ctx.events.is_empty());
    }

    #[test]
    fn cli_name_is_fallback_when_journal_has_no_prompt() {
        let mut f = fixture();
        // Journal whose only user line is system-like (skipped by the name rule).
        write_journal(
            &f.projects,
            "/home/u/p",
            "sid-1",
            &[r#"{"message":{"role":"user","content":"<system>boot</system>"}}"#],
        );
        *f.agents.lock().unwrap() = vec![cli_agent(7, "/home/u/p", "sid-1", "busy")];
        let mut ctx = Ctx::new();
        ctx.by_dir.push(("/home/u/p".into(), "p".into()));
        f.watcher.scan(&mut ctx, 1_000);
        assert_eq!(ctx.events[0].thread_name.as_deref(), Some("slug-7"));
    }

    #[test]
    fn shrunk_journal_resets_and_rederives() {
        let mut f = fixture();
        let path = write_journal(&f.projects, "/home/u/p", "sid-1", &[USER_LINE, RUNNING_LINE]);
        *f.agents.lock().unwrap() = vec![cli_agent(7, "/home/u/p", "sid-1", "busy")];
        let mut ctx = Ctx::new();
        ctx.by_dir.push(("/home/u/p".into(), "p".into()));
        f.watcher.scan(&mut ctx, 1_000);

        // Truncate + rewrite with a different prompt: state re-derives.
        let replacement = r#"{"message":{"role":"user","content":"a brand new thread"}}"#;
        std::fs::write(&path, format!("{replacement}\n")).unwrap();
        f.watcher.scan(&mut ctx, 3_000);
        let last = ctx.events.last().unwrap();
        assert_eq!(last.thread_name.as_deref(), Some("a brand new thread"));
    }

    #[test]
    fn journal_found_by_probe_when_encoding_guess_misses() {
        let mut f = fixture();
        // Dir name Claude-encoded from a path with a dot: guess `/`→`-` misses.
        let dir = f.projects.join("-home-u-my-app"); // actual dir for /home/u/my.app
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sid-1.jsonl"), format!("{USER_LINE}\n")).unwrap();
        *f.agents.lock().unwrap() = vec![cli_agent(7, "/home/u/my.app", "sid-1", "busy")];
        let mut ctx = Ctx::new();
        ctx.by_dir.push(("/home/u/my.app".into(), "p".into()));
        f.watcher.scan(&mut ctx, 1_000);
        assert_eq!(ctx.events[0].thread_name.as_deref(), Some("fix the flaky test"));
    }
}
