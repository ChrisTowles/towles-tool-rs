//! Claude Code agent watcher. Ports slot-1
//! `runtime/agents/watchers/claude-code.ts` per docs/AGENTBOARD-WATCHER-SPEC.md.
//!
//! Watches `~/.claude/projects/<encoded-dir>/<thread>.jsonl`, derives status from
//! journal entries, and emits [`AgentEvent`]s. The 2s scan is driven externally
//! (`scan(ctx, now_ms)`); fs-notify is an optional accelerant ([`crate::fs_notify`]).
//!
//! Three adopted fixes over the TS (see the spec's "Port decisions"):
//! 1. Offset tracked at the last newline boundary (re-reads an incomplete tail
//!    next tick); a shrunk file resets offset to 0 and re-seeds the thread.
//! 2. A usage delta is part of the Branch-C emit gate (token/model updates
//!    broadcast without needing a status change).
//! 3. Project dirs are matched encoded↔encoded: the raw encoded dir is carried
//!    through and handed to `resolve_session` (which re-encodes known paths),
//!    instead of the lossy decode.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Deserialize;

use crate::types::{AgentEvent, AgentEventDetails, AgentStatus, LoopInfo, SubagentInfo};
use crate::watcher::{AgentWatcher, JSONL_SUFFIX, STALE_MS, WatcherContext};
use crate::watchers::claude_pid::ClaudePidLookup;
use crate::watchers::claude_usage::{ClaudeUsageSummary, RawUsage, extract_usage_summary};

const NAME: &str = "claude-code";
const JOURNAL_IDLE_TIMEOUT_MS: i64 = crate::types::JOURNAL_IDLE_TIMEOUT_MS;

// --- Tolerant journal-entry types ---

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

// --- Pure status/metadata derivation (§3) ---

/// Derive a status from one entry, or `None` (ignored) — see the §3 decision table.
pub fn determine_status(entry: &RawEntry) -> Option<AgentStatus> {
    let msg = entry.message.as_ref()?;
    let role = msg.role.as_deref().filter(|r| !r.is_empty())?;
    let items = msg.content.as_ref().map(RawContent::items).unwrap_or(&[]);

    match role {
        "assistant" => {
            let tool_uses: Vec<&RawContentItem> =
                items.iter().filter(|c| c.item_type.as_deref() == Some("tool_use")).collect();
            if tool_uses.is_empty() {
                return Some(AgentStatus::Done);
            }
            let all_asking = tool_uses.iter().all(|c| c.name.as_deref() == Some("AskUserQuestion"));
            Some(if all_asking { AgentStatus::Question } else { AgentStatus::Running })
        }
        "user" => Some(AgentStatus::Running),
        _ => None,
    }
}

/// The thread name from the first qualifying user message (skips system-like
/// `<…>`/`{…}` first lines). Ports `extractThreadName`.
fn extract_thread_name(entry: &RawEntry) -> Option<String> {
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
        if now_ms - mtime > JOURNAL_IDLE_TIMEOUT_MS {
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

/// Fold entries carrying forward latest status and first thread name. Ports `scanEntries`.
fn scan_entries(
    entries: &[RawEntry],
    start_status: AgentStatus,
    start_thread_name: Option<String>,
) -> (AgentStatus, Option<String>) {
    let mut status = start_status;
    let mut thread_name = start_thread_name;
    for entry in entries {
        if thread_name.is_none()
            && let Some(name) = extract_thread_name(entry)
        {
            thread_name = Some(name);
        }
        if let Some(s) = determine_status(entry) {
            status = s;
        }
    }
    (status, thread_name)
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
    mtime_ms_from_meta(&meta)
}

fn mtime_ms_from_meta(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

// --- Details assembly (§6) ---

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

// --- Per-thread state ---

#[derive(Debug, Clone)]
struct SessionState {
    status: AgentStatus,
    /// Next read offset = last-newline byte boundary (adopted fix #1).
    file_size: u64,
    thread_name: Option<String>,
    /// Raw *encoded* project-dir name (adopted fix #3).
    project_dir: Option<String>,
    usage: Option<ClaudeUsageSummary>,
    last_tool: Option<String>,
    subagents: Vec<SubagentInfo>,
    subagent_sig: String,
    loop_state: Option<LoopInfo>,
}

// --- Watcher ---

/// The claude-code watcher. Ports `ClaudeCodeAgentWatcher`, minus the internal
/// timer/fs-watch (the caller drives [`AgentWatcher::scan`]).
pub struct ClaudeCodeAgentWatcher {
    projects_dir: PathBuf,
    sessions: HashMap<String, SessionState>,
    seeded: bool,
    pid_lookup: ClaudePidLookup,
}

impl ClaudeCodeAgentWatcher {
    /// Create rooted at `projects_dir`, using `pid_lookup` for liveness.
    pub fn new(projects_dir: PathBuf, pid_lookup: ClaudePidLookup) -> Self {
        Self { projects_dir, sessions: HashMap::new(), seeded: false, pid_lookup }
    }

    /// Create using the real `~/.claude/projects` + `~/.claude/sessions` locations.
    pub fn with_defaults() -> Self {
        let projects_dir =
            dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude").join("projects");
        Self::new(projects_dir, ClaudePidLookup::new(ClaudePidLookup::default_dir()))
    }

    /// Whether the first full scan has completed.
    pub fn is_seeded(&self) -> bool {
        self.seeded
    }

    fn emit(
        ctx: &mut dyn WatcherContext,
        session: String,
        status: AgentStatus,
        now_ms: i64,
        thread_id: &str,
        thread_name: Option<String>,
        details: Option<AgentEventDetails>,
    ) {
        ctx.emit(AgentEvent {
            agent: NAME.to_string(),
            session,
            status,
            ts: now_ms,
            thread_id: Some(thread_id.to_string()),
            thread_name,
            unseen: None,
            pane_id: None,
            details,
        });
    }

    fn process_file(
        &mut self,
        ctx: &mut dyn WatcherContext,
        file_path: &Path,
        encoded_dir: &str,
        now_ms: i64,
    ) {
        let Ok(meta) = std::fs::metadata(file_path) else {
            return;
        };
        let size = meta.len();
        let file_mtime = mtime_ms_from_meta(&meta);
        let thread_id = file_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let prev = self.sessions.get(&thread_id).cloned();

        // Sub-agents write to a sibling dir while the parent journal can stay static
        // for minutes — compute on every call, not just on growth.
        let subagents_dir = file_path
            .to_str()
            .and_then(|s| s.strip_suffix(JSONL_SUFFIX))
            .map(|base| PathBuf::from(format!("{base}/subagents")))
            .unwrap_or_default();
        let subagents = read_active_subagents(&subagents_dir, now_ms);
        let subagent_sig = subagent_signature(&subagents);

        // --- Branch A: no growth ---
        if let Some(prev) = &prev
            && size == prev.file_size
        {
            if self.seeded && prev.status == AgentStatus::Running {
                let pid = self.pid_lookup.pid_for_thread(&thread_id);
                let process_gone = pid.is_some_and(|p| !self.pid_lookup.is_alive(p));
                let mtime_stale = file_mtime.is_some_and(|m| now_ms - m > JOURNAL_IDLE_TIMEOUT_MS);
                if process_gone || mtime_stale {
                    let session = prev.project_dir.as_deref().and_then(|d| ctx.resolve_session(d));
                    let details = build_details(
                        prev.usage.as_ref(),
                        prev.last_tool.as_deref(),
                        &subagents,
                        prev.loop_state.as_ref(),
                    );
                    let thread_name = prev.thread_name.clone();
                    let st = self.sessions.get_mut(&thread_id).unwrap();
                    st.status = AgentStatus::Idle;
                    st.subagents = subagents;
                    st.subagent_sig = subagent_sig;
                    if let Some(session) = session {
                        Self::emit(
                            ctx,
                            session,
                            AgentStatus::Idle,
                            now_ms,
                            &thread_id,
                            thread_name,
                            details,
                        );
                    }
                    return;
                }
            }

            // Journal static but the live sub-agent set may have shifted.
            if subagent_sig != prev.subagent_sig {
                let session = prev.project_dir.as_deref().and_then(|d| ctx.resolve_session(d));
                let details = build_details(
                    prev.usage.as_ref(),
                    prev.last_tool.as_deref(),
                    &subagents,
                    prev.loop_state.as_ref(),
                );
                let status = prev.status;
                let thread_name = prev.thread_name.clone();
                let st = self.sessions.get_mut(&thread_id).unwrap();
                st.subagents = subagents;
                st.subagent_sig = subagent_sig;
                if let Some(session) = session {
                    Self::emit(ctx, session, status, now_ms, &thread_id, thread_name, details);
                }
            }
            return;
        }

        // --- Branch B: seed mode (whole file, no emit) ---
        if !self.seeded {
            let Ok(bytes) = std::fs::read(file_path) else {
                return;
            };
            let consumed = consumed_len(&bytes);
            let text = String::from_utf8_lossy(&bytes[..consumed]);
            let parsed = parse_journal_lines(&text);
            let (mut status, thread_name) = scan_entries(&parsed, AgentStatus::Idle, None);
            let usage = extract_usage_summary(&parsed);
            let last_tool = extract_last_tool(&parsed);
            let loop_state = extract_loop_state(&parsed);
            if status == AgentStatus::Running
                && file_mtime.is_some_and(|m| now_ms - m > JOURNAL_IDLE_TIMEOUT_MS)
            {
                status = AgentStatus::Idle;
            }
            self.sessions.insert(
                thread_id,
                SessionState {
                    status,
                    file_size: consumed as u64,
                    thread_name,
                    project_dir: Some(encoded_dir.to_string()),
                    usage,
                    last_tool,
                    subagents,
                    subagent_sig,
                    loop_state,
                },
            );
            return;
        }

        // --- Branch C: seeded & size changed ---
        let prev = prev.as_ref();
        // Adopted fix #1: a shrunk file (or a brand-new post-seed file) reads from 0
        // and re-seeds fresh; otherwise resume from the stored newline boundary.
        let (offset, reset) = match prev {
            Some(p) if size >= p.file_size => (p.file_size, false),
            _ => (0u64, true),
        };
        if !reset && size <= offset {
            return;
        }

        let Ok(bytes) = std::fs::read(file_path) else {
            return;
        };
        let start = (offset as usize).min(bytes.len());
        let end = (size as usize).min(bytes.len());
        let slice = if start <= end { &bytes[start..end] } else { &[][..] };
        let consumed = consumed_len(slice);
        let text = String::from_utf8_lossy(&slice[..consumed]);
        let parsed = parse_journal_lines(&text);

        let start_status = if reset { AgentStatus::Idle } else { prev.unwrap().status };
        let start_thread_name = if reset { None } else { prev.unwrap().thread_name.clone() };
        let (mut status, thread_name) = scan_entries(&parsed, start_status, start_thread_name);

        let usage = extract_usage_summary(&parsed).or_else(|| prev.and_then(|p| p.usage.clone()));
        let last_tool =
            extract_last_tool(&parsed).or_else(|| prev.and_then(|p| p.last_tool.clone()));
        let loop_state =
            extract_loop_state(&parsed).or_else(|| prev.and_then(|p| p.loop_state.clone()));

        if status == AgentStatus::Running {
            let pid = self.pid_lookup.pid_for_thread(&thread_id);
            if pid.is_some_and(|p| !self.pid_lookup.is_alive(p)) {
                status = AgentStatus::Idle;
            }
        }

        let prev_status = prev.map(|p| p.status);
        let prev_sig = prev.map(|p| p.subagent_sig.clone()).unwrap_or_default();
        let prev_loop_wake = prev.and_then(|p| p.loop_state.as_ref().map(|l| l.next_wake_at));
        let prev_usage = prev.and_then(|p| p.usage.clone());

        self.sessions.insert(
            thread_id.clone(),
            SessionState {
                status,
                file_size: offset + consumed as u64,
                thread_name: thread_name.clone(),
                project_dir: Some(encoded_dir.to_string()),
                usage: usage.clone(),
                last_tool: last_tool.clone(),
                subagents: subagents.clone(),
                subagent_sig: subagent_sig.clone(),
                loop_state: loop_state.clone(),
            },
        );

        let cur_loop_wake = loop_state.as_ref().map(|l| l.next_wake_at);
        let changed = Some(status) != prev_status
            || subagent_sig != prev_sig
            || cur_loop_wake != prev_loop_wake
            || usage != prev_usage; // adopted fix #2

        if changed && let Some(session) = ctx.resolve_session(encoded_dir) {
            let details = build_details(
                usage.as_ref(),
                last_tool.as_deref(),
                &subagents,
                loop_state.as_ref(),
            );
            Self::emit(ctx, session, status, now_ms, &thread_id, thread_name, details);
        }
    }

    /// On the first scan's completion, emit each stored non-idle session (re-checking
    /// running-liveness). Ports the seed-finalize block. Emits land while the server's
    /// `watchersSeeded` is false, so the bridge marks them unseen.
    fn seed_finalize(&mut self, ctx: &mut dyn WatcherContext, now_ms: i64) {
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for thread_id in ids {
            let st = &self.sessions[&thread_id];
            if st.status == AgentStatus::Idle || st.project_dir.is_none() {
                continue;
            }
            let mut status = st.status;
            let project_dir = st.project_dir.clone().unwrap();
            let thread_name = st.thread_name.clone();
            let details = build_details(
                st.usage.as_ref(),
                st.last_tool.as_deref(),
                &st.subagents,
                st.loop_state.as_ref(),
            );

            if status == AgentStatus::Running {
                let pid = self.pid_lookup.pid_for_thread(&thread_id);
                if pid.is_some_and(|p| !self.pid_lookup.is_alive(p)) {
                    self.sessions.get_mut(&thread_id).unwrap().status = AgentStatus::Idle;
                    continue;
                }
                status = AgentStatus::Running;
            }

            if let Some(session) = ctx.resolve_session(&project_dir) {
                Self::emit(ctx, session, status, now_ms, &thread_id, thread_name, details);
            }
        }
    }
}

impl AgentWatcher for ClaudeCodeAgentWatcher {
    fn name(&self) -> &str {
        NAME
    }

    fn scan(&mut self, ctx: &mut dyn WatcherContext, now_ms: i64) {
        self.pid_lookup.invalidate();

        if let Ok(entries) = std::fs::read_dir(&self.projects_dir) {
            for entry in entries.flatten() {
                let dir_path = entry.path();
                match std::fs::metadata(&dir_path) {
                    Ok(m) if m.is_dir() => {}
                    _ => continue,
                }
                let encoded_dir = entry.file_name().to_string_lossy().to_string();
                let Ok(files) = std::fs::read_dir(&dir_path) else {
                    continue;
                };
                for file in files.flatten() {
                    let file_path = file.path();
                    if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let Some(mtime) = mtime_ms(&file_path) else {
                        continue;
                    };
                    if now_ms - mtime > STALE_MS {
                        continue;
                    }
                    self.process_file(ctx, &file_path, &encoded_dir, now_ms);
                }
            }
        }

        // Seed finalize runs even if the projects dir was unreadable (matches the
        // TS try/finally), flipping `seeded` after the first scan.
        if !self.seeded {
            self.seeded = true;
            self.seed_finalize(ctx, now_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    fn entry(json: serde_json::Value) -> RawEntry {
        serde_json::from_value(json).unwrap()
    }

    fn now_real_ms() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    }

    // --- §3 status decision table ---

    #[test]
    fn status_table() {
        let user = entry(serde_json::json!({"message":{"role":"user","content":"hi"}}));
        assert_eq!(determine_status(&user), Some(AgentStatus::Running));

        let text_only = entry(
            serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}),
        );
        assert_eq!(determine_status(&text_only), Some(AgentStatus::Done));

        let string_content =
            entry(serde_json::json!({"message":{"role":"assistant","content":"done thinking"}}));
        assert_eq!(determine_status(&string_content), Some(AgentStatus::Done));

        let asking = entry(serde_json::json!({"message":{"role":"assistant","content":[
            {"type":"tool_use","name":"AskUserQuestion"}]}}));
        assert_eq!(determine_status(&asking), Some(AgentStatus::Question));

        let mixed = entry(serde_json::json!({"message":{"role":"assistant","content":[
            {"type":"tool_use","name":"AskUserQuestion"},{"type":"tool_use","name":"Bash"}]}}));
        assert_eq!(determine_status(&mixed), Some(AgentStatus::Running));

        let no_role = entry(serde_json::json!({"message":{"content":"x"}}));
        assert_eq!(determine_status(&no_role), None);

        let system = entry(serde_json::json!({"message":{"role":"system","content":"x"}}));
        assert_eq!(determine_status(&system), None);
    }

    #[test]
    fn thinking_only_reads_as_done() {
        // A `thinking` content block has no tool_use and no text → done (§3 quirk kept).
        let thinking = entry(
            serde_json::json!({"message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"}]}}),
        );
        assert_eq!(determine_status(&thinking), Some(AgentStatus::Done));
    }

    #[test]
    fn thread_name_skips_system_like_and_caps_80() {
        let sys =
            entry(serde_json::json!({"message":{"role":"user","content":"<command>x</command>"}}));
        assert_eq!(extract_thread_name(&sys), None);
        let braces =
            entry(serde_json::json!({"message":{"role":"user","content":"{tool_result}"}}));
        assert_eq!(extract_thread_name(&braces), None);
        let long = "a".repeat(100);
        let e = entry(serde_json::json!({"message":{"role":"user","content": long}}));
        assert_eq!(extract_thread_name(&e).unwrap().chars().count(), 80);
    }

    #[test]
    fn last_tool_newest_first_skipping_ask() {
        let entries = vec![
            entry(
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"tool_use","name":"Read"}]}}),
            ),
            entry(
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"tool_use","name":"AskUserQuestion"}]}}),
            ),
        ];
        // Newest is AskUserQuestion (skipped) → falls back to Read.
        assert_eq!(extract_last_tool(&entries).as_deref(), Some("Read"));
    }

    #[test]
    fn loop_state_from_schedule_wakeup() {
        let entries = vec![entry(serde_json::json!({
            "timestamp": "2026-04-12T00:00:00Z",
            "message": {"role":"assistant","content":[
                {"type":"tool_use","name":"ScheduleWakeup","input":{"delaySeconds":90,"reason":"poll CI"}}]}
        }))];
        let loop_ = extract_loop_state(&entries).unwrap();
        let base = parse_timestamp_ms("2026-04-12T00:00:00Z").unwrap();
        assert_eq!(loop_.next_wake_at, base + 90_000);
        assert_eq!(loop_.reason.as_deref(), Some("poll CI"));
    }

    #[test]
    fn subagent_signature_is_order_independent() {
        let a = SubagentInfo { agent_type: Some("Explore".into()), description: Some("x".into()) };
        let b = SubagentInfo { agent_type: Some("Plan".into()), description: Some("y".into()) };
        assert_eq!(subagent_signature(&[a.clone(), b.clone()]), subagent_signature(&[b, a]));
    }

    // --- §4 subagents ---

    #[test]
    fn read_active_subagents_window_meta_and_sort() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        // active agent with meta
        std::fs::write(sub.join("agent-1.jsonl"), "{}\n").unwrap();
        std::fs::write(
            sub.join("agent-1.meta.json"),
            serde_json::json!({"agentType":"Explore","description":"search"}).to_string(),
        )
        .unwrap();
        // active agent without meta (counts as {})
        std::fs::write(sub.join("agent-2.jsonl"), "{}\n").unwrap();
        // non-agent file ignored
        std::fs::write(sub.join("notes.jsonl"), "{}\n").unwrap();

        let now = now_real_ms();
        let active = read_active_subagents(&sub, now);
        assert_eq!(active.len(), 2);
        assert!(active.iter().any(|s| s.agent_type.as_deref() == Some("Explore")));
        assert!(active.iter().any(|s| s.agent_type.is_none()));

        // Far-future now → all stale → skipped.
        let stale = read_active_subagents(&sub, now + 3 * 60_000);
        assert!(stale.is_empty());

        // Missing dir → empty.
        assert!(read_active_subagents(&dir.path().join("nope"), now).is_empty());
    }

    // --- Watcher scan flow ---

    struct RecordingCtx {
        events: Vec<AgentEvent>,
        resolve: HashMap<String, String>,
    }
    impl RecordingCtx {
        fn new(encoded_dir: &str, session: &str) -> Self {
            let mut resolve = HashMap::new();
            resolve.insert(encoded_dir.to_string(), session.to_string());
            Self { events: Vec::new(), resolve }
        }
    }
    impl WatcherContext for RecordingCtx {
        fn resolve_session(&self, project_dir: &str) -> Option<String> {
            self.resolve.get(project_dir).cloned()
        }
        fn emit(&mut self, event: AgentEvent) {
            self.events.push(event);
        }
    }

    const ENC: &str = "-home-u-proj";
    const THREAD: &str = "sess1";

    fn write_journal(projects: &Path, lines: &[serde_json::Value]) {
        let dir = projects.join(ENC);
        std::fs::create_dir_all(&dir).unwrap();
        let mut body = String::new();
        for l in lines {
            body.push_str(&l.to_string());
            body.push('\n');
        }
        std::fs::write(dir.join(format!("{THREAD}.jsonl")), body).unwrap();
    }

    fn new_watcher(
        projects: &Path,
        sessions: &Path,
        alive: fn(i32) -> bool,
    ) -> ClaudeCodeAgentWatcher {
        ClaudeCodeAgentWatcher::new(
            projects.to_path_buf(),
            ClaudePidLookup::with_is_alive(sessions.to_path_buf(), alive),
        )
    }

    #[test]
    fn empty_file_is_idle_no_emit() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        write_journal(projects.path(), &[]);
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "my-session");
        w.scan(&mut ctx, now_real_ms());
        assert!(ctx.events.is_empty());
    }

    #[test]
    fn seed_finalize_emits_non_idle() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        // assistant text-only → done.
        write_journal(
            projects.path(),
            &[
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"all done"}]}}),
            ],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "my-session");
        w.scan(&mut ctx, now_real_ms());
        assert_eq!(ctx.events.len(), 1);
        assert_eq!(ctx.events[0].status, AgentStatus::Done);
        assert_eq!(ctx.events[0].session, "my-session");
        assert_eq!(ctx.events[0].thread_id.as_deref(), Some(THREAD));
    }

    #[test]
    fn branch_c_emits_on_status_change() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        // Seed: a user turn → running.
        write_journal(
            projects.path(),
            &[serde_json::json!({"message":{"role":"user","content":"do it"}})],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        let now = now_real_ms();
        w.scan(&mut ctx, now); // seed + finalize (running emitted by seed finalize)
        ctx.events.clear();
        // Append an assistant done turn.
        write_journal(
            projects.path(),
            &[
                serde_json::json!({"message":{"role":"user","content":"do it"}}),
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}),
            ],
        );
        w.scan(&mut ctx, now);
        assert_eq!(ctx.events.len(), 1);
        assert_eq!(ctx.events[0].status, AgentStatus::Done);
    }

    #[test]
    fn partial_line_across_reads_is_not_lost() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        let dir = projects.path().join(ENC);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{THREAD}.jsonl"));
        // Seed with a complete user line.
        std::fs::write(&file, "{\"message\":{\"role\":\"user\",\"content\":\"go\"}}\n").unwrap();
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        let now = now_real_ms();
        w.scan(&mut ctx, now);
        ctx.events.clear();

        // Append a PARTIAL assistant line (no trailing newline yet).
        let partial = "{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"tex";
        std::fs::write(
            &file,
            format!("{{\"message\":{{\"role\":\"user\",\"content\":\"go\"}}}}\n{partial}"),
        )
        .unwrap();
        w.scan(&mut ctx, now);
        assert!(ctx.events.is_empty(), "partial line must not be processed yet");

        // Complete the line.
        let full_second = "{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}";
        std::fs::write(
            &file,
            format!("{{\"message\":{{\"role\":\"user\",\"content\":\"go\"}}}}\n{full_second}\n"),
        )
        .unwrap();
        w.scan(&mut ctx, now);
        assert_eq!(ctx.events.len(), 1);
        assert_eq!(ctx.events[0].status, AgentStatus::Done);
    }

    #[test]
    fn truncation_resets_and_recovers() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        // Seed with a long assistant-done journal.
        write_journal(
            projects.path(),
            &[
                serde_json::json!({"message":{"role":"user","content":"go"}}),
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"a very long completed answer here"}]}}),
            ],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        let now = now_real_ms();
        w.scan(&mut ctx, now);
        ctx.events.clear();
        // Truncate to a shorter file with a single running user turn.
        write_journal(
            projects.path(),
            &[serde_json::json!({"message":{"role":"user","content":"x"}})],
        );
        w.scan(&mut ctx, now);
        assert_eq!(ctx.events.len(), 1, "should re-seed and emit after truncation");
        assert_eq!(ctx.events[0].status, AgentStatus::Running);
    }

    #[test]
    fn question_status_emitted() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        write_journal(
            projects.path(),
            &[
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"tool_use","name":"AskUserQuestion"}]}}),
            ],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        w.scan(&mut ctx, now_real_ms());
        assert_eq!(ctx.events.len(), 1);
        assert_eq!(ctx.events[0].status, AgentStatus::Question);
    }

    #[test]
    fn loop_details_ride_along_on_emit() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        write_journal(
            projects.path(),
            &[serde_json::json!({
                "timestamp":"2026-04-12T00:00:00Z",
                "message":{"role":"assistant","content":[
                    {"type":"tool_use","name":"ScheduleWakeup","input":{"delaySeconds":120,"reason":"wait"}}]}
            })],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        w.scan(&mut ctx, now_real_ms());
        assert_eq!(ctx.events.len(), 1);
        let details = ctx.events[0].details.as_ref().unwrap();
        let base = parse_timestamp_ms("2026-04-12T00:00:00Z").unwrap();
        assert_eq!(details.r#loop.as_ref().unwrap().next_wake_at, base + 120_000);
    }

    #[test]
    fn subagents_reflected_in_details() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        write_journal(
            projects.path(),
            &[
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}),
            ],
        );
        // Add an active subagent alongside the thread journal.
        let sub = projects.path().join(ENC).join(THREAD).join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("agent-1.jsonl"), "{}\n").unwrap();
        std::fs::write(
            sub.join("agent-1.meta.json"),
            serde_json::json!({"agentType":"Explore","description":"scan"}).to_string(),
        )
        .unwrap();

        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        w.scan(&mut ctx, now_real_ms());
        let details = ctx.events[0].details.as_ref().unwrap();
        let subs = details.subagents.as_ref().unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].agent_type.as_deref(), Some("Explore"));
    }

    #[test]
    fn pid_dead_demotes_running_to_idle_branch_c() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        // Map the thread to a pid, and make everything "dead".
        std::fs::write(
            sessions.path().join("4242.json"),
            serde_json::json!({"pid":4242,"sessionId":THREAD}).to_string(),
        )
        .unwrap();
        // Seed: assistant done (a terminal status, not demoted at seed).
        write_journal(
            projects.path(),
            &[
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}),
            ],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| false);
        let mut ctx = RecordingCtx::new(ENC, "s");
        let now = now_real_ms();
        w.scan(&mut ctx, now); // seed stores done; finalize emits done
        ctx.events.clear();
        // Grow with a user turn (→ running), but the mapped pid is dead → idle.
        write_journal(
            projects.path(),
            &[
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}),
                serde_json::json!({"message":{"role":"user","content":"more"}}),
            ],
        );
        w.scan(&mut ctx, now);
        assert_eq!(ctx.events.len(), 1);
        assert_eq!(ctx.events[0].status, AgentStatus::Idle);
    }

    #[test]
    fn branch_a_mtime_stale_demotes_running() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        write_journal(
            projects.path(),
            &[serde_json::json!({"message":{"role":"user","content":"go"}})],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        let now = now_real_ms();
        w.scan(&mut ctx, now); // seed → running stored; finalize emits running
        ctx.events.clear();
        // Same file (no growth), but now far in the future → journal mtime stale.
        w.scan(&mut ctx, now + 3 * 60_000);
        assert_eq!(ctx.events.len(), 1);
        assert_eq!(ctx.events[0].status, AgentStatus::Idle);
    }

    #[test]
    fn usage_delta_emits_without_status_change() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        // Seed: assistant done with usage A.
        write_journal(
            projects.path(),
            &[serde_json::json!({
                "timestamp":"2026-04-12T00:00:00Z",
                "message":{"role":"assistant","model":"claude-opus-4-6","content":[{"type":"text","text":"a"}],
                    "usage":{"input_tokens":10,"output_tokens":5}}
            })],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        let now = now_real_ms();
        w.scan(&mut ctx, now);
        ctx.events.clear();
        // Append another done turn (status stays done) but with different usage.
        write_journal(
            projects.path(),
            &[
                serde_json::json!({
                    "timestamp":"2026-04-12T00:00:00Z",
                    "message":{"role":"assistant","model":"claude-opus-4-6","content":[{"type":"text","text":"a"}],
                        "usage":{"input_tokens":10,"output_tokens":5}}
                }),
                serde_json::json!({
                    "timestamp":"2026-04-12T00:05:00Z",
                    "message":{"role":"assistant","model":"claude-opus-4-6","content":[{"type":"text","text":"b"}],
                        "usage":{"input_tokens":999,"output_tokens":42}}
                }),
            ],
        );
        w.scan(&mut ctx, now);
        assert_eq!(ctx.events.len(), 1, "usage delta alone should emit (adopted fix #2)");
        assert_eq!(ctx.events[0].status, AgentStatus::Done);
        assert_eq!(ctx.events[0].details.as_ref().unwrap().context_used, Some(1041));
    }

    #[test]
    fn stale_file_skipped_entirely() {
        let projects = TempDir::new().unwrap();
        let sessions = TempDir::new().unwrap();
        write_journal(
            projects.path(),
            &[
                serde_json::json!({"message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}),
            ],
        );
        let mut w = new_watcher(projects.path(), sessions.path(), |_| true);
        let mut ctx = RecordingCtx::new(ENC, "s");
        // now far ahead of the file mtime → older than STALE_MS (5min) → skipped.
        w.scan(&mut ctx, now_real_ms() + 10 * 60_000);
        assert!(ctx.events.is_empty());
    }
}
