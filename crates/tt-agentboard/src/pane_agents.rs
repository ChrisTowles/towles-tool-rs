//! Pane↔agent attribution (phase T6 of docs/AGENTBOARD-TMUX-SPEC.md). Ports
//! the pane-scanning/resolution half of slot-1 `server/index.ts`:
//! `scanAllTmuxPaneAgents`, `resolveAgentPaneId`, the per-agent pane-info
//! resolvers, `buildProcessTree`/`matchProcessTreeFast`/`findChildPidFast`,
//! `paneAgentSetsDiffer`, and `mergeAgentsWithPanePresence` /
//! `overrideTerminalIfPaneAlive`.
//!
//! Claude Code resolution deviates from the TS by design (Chris, 2026-07-03):
//! instead of reading `~/.claude/sessions/<pid>.json` per pane and re-parsing
//! journal JSONL for thread name/status, one `claude agents --all --json`
//! call per scan yields pid → {sessionId, name, status} for every live
//! Claude. The incremental journal *watcher* is unaffected — it still
//! supplies the rich details (model, tool, cache, subagents) the CLI doesn't.
//!
//! House rule: parsing and merge logic are pure and fixture-tested;
//! subprocess/sqlite/lsof access lives in thin, un-unit-tested functions.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use indexmap::IndexMap;

use crate::types::{AgentEvent, AgentStatus};

/// Agent name → lowercase process/title substrings that identify it.
pub const AGENT_TITLE_PATTERNS: &[(&str, &[&str])] = &[
    ("amp", &["amp"]),
    ("claude-code", &["claude"]),
    ("codex", &["codex"]),
    ("opencode", &["opencode"]),
];

fn patterns_for(agent: &str) -> Option<&'static [&'static str]> {
    AGENT_TITLE_PATTERNS.iter().find(|(name, _)| *name == agent).map(|(_, p)| *p)
}

/// An agent detected inside a live tmux pane.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneAgentPresence {
    pub agent: String,
    pub session: String,
    pub pane_id: String,
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
    pub status: Option<AgentStatus>,
    pub last_seen_ts: i64,
}

/// Presence maps: session name → (instance key `agent:pane:<id>` → presence).
pub type PaneAgentMap = IndexMap<String, IndexMap<String, PaneAgentPresence>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneEntry {
    pub session: String,
    pub id: String,
    pub pid: i32,
    pub cmd: String,
    pub title: String,
}

/// Format string for the pane listings this module consumes.
pub const PANE_SCAN_FORMAT: &str =
    "#{session_name}|#{pane_id}|#{pane_pid}|#{pane_current_command}|#{pane_title}";

/// Parse `session|pane_id|pid|cmd|title` lines (title may contain `|`).
pub fn parse_pane_lines(raw: &str) -> Vec<PaneEntry> {
    raw.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let mut parts = line.splitn(5, '|');
            let session = parts.next()?;
            let id = parts.next()?;
            let pid = parts.next()?.parse().unwrap_or(0);
            let cmd = parts.next()?;
            let title = parts.next().unwrap_or("");
            Some(PaneEntry {
                session: session.to_string(),
                id: id.to_string(),
                pid,
                cmd: cmd.to_string(),
                title: title.to_string(),
            })
        })
        .collect()
}

// --- Process tree (ports buildProcessTree / matchProcessTreeFast / findChildPidFast) ---

#[derive(Debug, Default)]
pub struct ProcessTree {
    pub children_of: HashMap<i32, Vec<i32>>,
    pub parent_of: HashMap<i32, i32>,
    pub comm_of: HashMap<i32, String>,
}

/// Parse one `ps -eo pid=,ppid=,comm=` snapshot.
pub fn parse_process_tree(raw: &str) -> ProcessTree {
    let mut tree = ProcessTree::default();
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<i32>(), ppid.parse::<i32>()) else {
            continue;
        };
        let comm = parts.collect::<Vec<_>>().join(" ").to_lowercase();
        if comm.is_empty() {
            continue;
        }
        tree.comm_of.insert(pid, comm);
        tree.parent_of.insert(pid, ppid);
        tree.children_of.entry(ppid).or_default().push(pid);
    }
    tree
}

/// Walk up to 3 levels of children looking for any pattern match.
pub fn match_process_tree(pid: i32, patterns: &[&str], tree: &ProcessTree, depth: u8) -> bool {
    if depth > 2 {
        return false;
    }
    let Some(children) = tree.children_of.get(&pid) else {
        return false;
    };
    for child in children {
        if let Some(comm) = tree.comm_of.get(child)
            && patterns.iter().any(|pat| comm.contains(pat))
        {
            return true;
        }
        if match_process_tree(*child, patterns, tree, depth + 1) {
            return true;
        }
    }
    false
}

/// Walk `pid`'s ancestry until a pid that is a tmux pane pid, returning that
/// pane's session (T7: attribute an agent to the tmux session owning it).
pub fn ancestor_pane_session(
    pid: i32,
    session_by_pane_pid: &HashMap<i32, String>,
    tree: &ProcessTree,
) -> Option<String> {
    let mut current = pid;
    for _ in 0..32 {
        if let Some(session) = session_by_pane_pid.get(&current) {
            return Some(session.clone());
        }
        current = *tree.parent_of.get(&current)?;
    }
    None
}

/// First child pid (≤3 levels deep) whose comm contains `name`.
pub fn find_child_pid(pid: i32, name: &str, tree: &ProcessTree, depth: u8) -> Option<i32> {
    if depth > 2 {
        return None;
    }
    let children = tree.children_of.get(&pid)?;
    for child in children {
        if tree.comm_of.get(child).is_some_and(|comm| comm.contains(name)) {
            return Some(*child);
        }
        if let Some(found) = find_child_pid(*child, name, tree, depth + 1) {
            return Some(found);
        }
    }
    None
}

// --- Per-agent info resolution (pure parts) ---

/// Amp pane title format: `amp - <threadName> - <dir>`.
pub fn resolve_amp_pane_info(title: &str) -> Option<String> {
    if !title.to_lowercase().starts_with("amp - ") {
        return None;
    }
    let rest = &title[6..];
    let thread_name = match rest.rfind(" - ") {
        Some(idx) if idx > 0 => &rest[..idx],
        _ => rest,
    };
    (!thread_name.is_empty()).then(|| thread_name.to_string())
}

// --- Presence assembly (pure; per-agent I/O injected) ---

/// Thread info resolved for a Claude Code pid: (thread_id, thread_name, status).
pub type ClaudeThreadInfo = (String, Option<String>, Option<AgentStatus>);

/// Callbacks that resolve thread info for a pane's agent child process.
/// Injected so the assembly stays pure and testable.
pub struct PaneResolvers<'a> {
    /// pid → (thread_id, thread_name, status) for Claude Code.
    pub claude: &'a dyn Fn(i32) -> Option<ClaudeThreadInfo>,
    /// pid → thread_id for Codex.
    pub codex: &'a dyn Fn(i32) -> Option<String>,
}

/// Ports `scanAllTmuxPaneAgents`'s assembly: process-tree matching only
/// (title matching produces false positives — e.g. an Amp thread named
/// "Detect Claude session names" matches "claude").
pub fn assemble_presences(
    panes: &[PaneEntry],
    tree: &ProcessTree,
    sidebar_pane_ids: &HashSet<String>,
    resolvers: &PaneResolvers<'_>,
    now_ms: i64,
) -> PaneAgentMap {
    let mut result: PaneAgentMap = IndexMap::new();

    for pane in panes.iter().filter(|p| !sidebar_pane_ids.contains(&p.id)) {
        for (agent_name, patterns) in AGENT_TITLE_PATTERNS {
            if !match_process_tree(pane.pid, patterns, tree, 0) {
                continue;
            }

            let mut thread_id = None;
            let mut thread_name = None;
            let mut status = None;
            match *agent_name {
                "amp" => thread_name = resolve_amp_pane_info(&pane.title),
                "claude-code" => {
                    if let Some(agent_pid) = find_child_pid(pane.pid, "claude", tree, 0)
                        && let Some((tid, tname, st)) = (resolvers.claude)(agent_pid)
                    {
                        thread_id = Some(tid);
                        thread_name = tname;
                        status = st;
                    }
                }
                "codex" => {
                    if let Some(agent_pid) = find_child_pid(pane.pid, "codex", tree, 0) {
                        thread_id = (resolvers.codex)(agent_pid);
                    }
                }
                _ => {}
            }

            let key = format!("{agent_name}:pane:{}", pane.id);
            result.entry(pane.session.clone()).or_default().insert(
                key,
                PaneAgentPresence {
                    agent: agent_name.to_string(),
                    session: pane.session.clone(),
                    pane_id: pane.id.clone(),
                    thread_id,
                    thread_name,
                    status,
                    last_seen_ts: now_ms,
                },
            );
        }
    }
    result
}

/// True when two pane-agent snapshots differ in sessions or instance keys.
pub fn pane_agent_sets_differ(prev: &PaneAgentMap, next: &PaneAgentMap) -> bool {
    if prev.len() != next.len() {
        return true;
    }
    for (session, agents) in next {
        let Some(prev_agents) = prev.get(session) else {
            return true;
        };
        if prev_agents.len() != agents.len() {
            return true;
        }
        for key in agents.keys() {
            if !prev_agents.contains_key(key) {
                return true;
            }
        }
    }
    false
}

// --- Snapshot merge (ports mergeAgentsWithPanePresence / overrideTerminalIfPaneAlive) ---

/// If the session's top-line state is terminal but a pane still runs that
/// agent instance, rewrite it to `idle` (alive at the prompt).
pub fn override_terminal_if_pane_alive(
    state: Option<AgentEvent>,
    pane_agents: Option<&IndexMap<String, PaneAgentPresence>>,
) -> Option<AgentEvent> {
    let mut state = state?;
    if !state.status.is_terminal() {
        return Some(state);
    }
    let Some(pane_agents) = pane_agents else {
        return Some(state);
    };
    for presence in pane_agents.values() {
        if presence.agent == state.agent && presence.thread_id == state.thread_id {
            state.status = AgentStatus::Idle;
            break;
        }
    }
    Some(state)
}

/// Merge pane-detected agents into watcher-provided agents for a session.
/// Watcher events take precedence — pane presence only adds synthetic
/// entries for agents the watchers don't track. Also drops non-terminal
/// watcher agents whose pane has closed (the tracker only prunes terminals
/// on a timeout, so waiting/running agents would otherwise linger forever
/// after their tmux pane is killed).
pub fn merge_agents_with_pane_presence(
    session_name: &str,
    watcher_agents: Vec<AgentEvent>,
    pane_agents: Option<&IndexMap<String, PaneAgentPresence>>,
) -> Vec<AgentEvent> {
    let has_instance = |agent: &str, thread_id: Option<&str>| -> bool {
        pane_agents.is_some_and(|pa| {
            pa.values().any(|p| p.agent == agent && p.thread_id.as_deref() == thread_id)
        })
    };

    let live_watcher_agents: Vec<AgentEvent> = watcher_agents
        .iter()
        .filter(|a| {
            if a.pane_id.is_none() || a.status.is_terminal() {
                return true;
            }
            has_instance(&a.agent, a.thread_id.as_deref())
        })
        .cloned()
        .collect();

    let Some(pane_agents) = pane_agents else {
        return live_watcher_agents;
    };
    if pane_agents.is_empty() {
        return live_watcher_agents;
    }

    let mut result = live_watcher_agents;
    for presence in pane_agents.values() {
        let tracked_idx = result.iter().position(|a| {
            a.agent == presence.agent && a.thread_id.as_deref() == presence.thread_id.as_deref()
        });

        if let Some(idx) = tracked_idx {
            // Watcher already tracks this agent — the process is confirmed
            // alive, so a terminal journal status means waiting for input.
            let tracked = &mut result[idx];
            if tracked.status.is_terminal() {
                tracked.status = AgentStatus::Idle;
                tracked.pane_id = Some(presence.pane_id.clone());
            }
            continue;
        }

        // No threadId from the pane scan + the watcher tracks some instance
        // of this agent → assume it's the same one; skip the synthetic row.
        if presence.thread_id.is_none() && watcher_agents.iter().any(|a| a.agent == presence.agent)
        {
            continue;
        }

        result.push(AgentEvent {
            agent: presence.agent.clone(),
            session: session_name.to_string(),
            status: presence.status.unwrap_or(AgentStatus::Idle),
            ts: presence.last_seen_ts,
            thread_id: presence.thread_id.clone(),
            thread_name: presence.thread_name.clone(),
            unseen: None,
            pane_id: Some(presence.pane_id.clone()),
            details: None,
        });
    }
    result
}

// --- Thin I/O wrappers (un-unit-tested by design) ---

/// All panes across every tmux session.
pub fn list_all_panes() -> Vec<PaneEntry> {
    match tt_exec::run("tmux", &["list-panes", "-a", "-F", PANE_SCAN_FORMAT]) {
        Ok(out) if out.ok() => parse_pane_lines(&out.stdout),
        _ => Vec::new(),
    }
}

/// All panes of one session.
pub fn list_session_panes(session: &str) -> Vec<PaneEntry> {
    match tt_exec::run("tmux", &["list-panes", "-s", "-t", session, "-F", PANE_SCAN_FORMAT]) {
        Ok(out) if out.ok() => parse_pane_lines(&out.stdout),
        _ => Vec::new(),
    }
}

/// One `ps` snapshot for the whole scan.
pub fn ps_tree() -> ProcessTree {
    match tt_exec::run("ps", &["-eo", "pid=,ppid=,comm="]) {
        Ok(out) if out.ok() => parse_process_tree(&out.stdout),
        _ => ProcessTree::default(),
    }
}

/// pid → live Claude info via the shared cached CLI snapshot.
pub fn claude_agents_by_pid() -> HashMap<i32, crate::claude_cli::CliAgent> {
    crate::claude_cli::fetch_agents_cached(Duration::from_millis(
        crate::watchers::claude_code::CLI_CACHE_TTL_MS,
    ))
    .into_iter()
    .map(|a| (a.pid, a))
    .collect()
}

/// Codex: latest thread_id for an agent pid from `$CODEX_HOME/logs_1.sqlite`.
pub fn codex_thread_for_pid(pid: i32) -> Option<String> {
    let home = std::env::var("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".codex"));
    let db_path = home.join("logs_1.sqlite");
    let db =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    db.query_row(
        "SELECT thread_id FROM logs WHERE process_uuid LIKE ?1 AND thread_id IS NOT NULL \
         ORDER BY ts DESC LIMIT 1",
        [format!("pid:{pid}:%")],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// OpenCode: does the agent pid's open log file mention `thread_id`?
/// (lsof → `/opencode/log/*.log` → grep `ses_*`.)
pub fn opencode_pane_matches(pid: i32, thread_id: &str) -> bool {
    let Ok(out) = tt_exec::run("lsof", &["-p", &pid.to_string()]) else {
        return false;
    };
    let Some(log_line) =
        out.stdout.lines().find(|l| l.contains("/opencode/log/") && l.ends_with(".log"))
    else {
        return false;
    };
    let Some(path) = log_line.split_whitespace().last().filter(|p| p.starts_with('/')) else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.split("ses_").nth(1).is_some_and(|rest| {
        let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        format!("ses_{id}") == thread_id
    })
}

/// Resolve a tmux pane ID for an agent, trying (in order): the agent-specific
/// thread resolution, amp title match, generic title match, process-tree
/// match. Ports `resolveAgentPaneId`.
pub fn resolve_agent_pane_id(
    session_name: &str,
    agent_name: &str,
    thread_id: Option<&str>,
    thread_name: Option<&str>,
    sidebar_pane_ids: &HashSet<String>,
) -> Option<String> {
    let patterns = patterns_for(agent_name)?;
    let panes = list_session_panes(session_name);
    let non_sidebar: Vec<&PaneEntry> =
        panes.iter().filter(|p| !sidebar_pane_ids.contains(&p.id)).collect();
    if non_sidebar.is_empty() {
        return None;
    }
    let tree = ps_tree();

    if agent_name == "claude-code"
        && let Some(tid) = thread_id
    {
        let by_pid = claude_agents_by_pid();
        for pane in &non_sidebar {
            if let Some(agent_pid) = find_child_pid(pane.pid, "claude", &tree, 0)
                && by_pid.get(&agent_pid).is_some_and(|a| a.session_id == tid)
            {
                return Some(pane.id.clone());
            }
        }
    }
    if agent_name == "amp"
        && let Some(tname) = thread_name
        && let Some(pane) = non_sidebar
            .iter()
            .find(|p| p.title.to_lowercase().starts_with("amp - ") && p.title.contains(tname))
    {
        return Some(pane.id.clone());
    }
    if agent_name == "codex"
        && let Some(tid) = thread_id
    {
        for pane in &non_sidebar {
            if let Some(agent_pid) = find_child_pid(pane.pid, "codex", &tree, 0)
                && codex_thread_for_pid(agent_pid).as_deref() == Some(tid)
            {
                return Some(pane.id.clone());
            }
        }
    }
    if agent_name == "opencode"
        && let Some(tid) = thread_id
    {
        for pane in &non_sidebar {
            if let Some(agent_pid) = find_child_pid(pane.pid, "opencode", &tree, 0)
                && opencode_pane_matches(agent_pid, tid)
            {
                return Some(pane.id.clone());
            }
        }
    }
    // Fallbacks: title substring, then process tree.
    if let Some(pane) =
        non_sidebar.iter().find(|p| patterns.iter().any(|pat| p.title.to_lowercase().contains(pat)))
    {
        return Some(pane.id.clone());
    }
    non_sidebar.iter().find(|p| match_process_tree(p.pid, patterns, &tree, 0)).map(|p| p.id.clone())
}

/// Scan all panes across all tmux sessions and identify running agents.
/// One `list-panes -a`, one `ps`, one `claude agents` call per scan.
pub fn scan_all_tmux_pane_agents(sidebar_pane_ids: &HashSet<String>, now_ms: i64) -> PaneAgentMap {
    let panes = list_all_panes();
    if panes.is_empty() {
        return IndexMap::new();
    }
    let tree = ps_tree();
    let claude_by_pid = claude_agents_by_pid();
    let resolvers = PaneResolvers {
        claude: &|pid| {
            claude_by_pid
                .get(&pid)
                .map(|a| (a.session_id.clone(), a.name.clone(), a.agent_status()))
        },
        codex: &codex_thread_for_pid,
    };
    assemble_presences(&panes, &tree, sidebar_pane_ids, &resolvers, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(edges: &[(i32, i32, &str)]) -> ProcessTree {
        // edges: (pid, ppid, comm)
        let raw: String =
            edges.iter().map(|(pid, ppid, comm)| format!(" {pid}  {ppid} {comm}\n")).collect();
        parse_process_tree(&raw)
    }

    #[test]
    fn parses_pane_lines_with_pipes_in_title() {
        let raw = "main|%1|100|zsh|my | piped | title\nother|%2|200|nvim|plain";
        let panes = parse_pane_lines(raw);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].title, "my | piped | title");
        assert_eq!(panes[0].pid, 100);
        assert_eq!(panes[1].session, "other");
    }

    #[test]
    fn process_tree_matches_within_three_levels() {
        let t = tree(&[
            (100, 1, "zsh"),
            (200, 100, "node"),
            (300, 200, "claude"),
            (400, 300, "deep"),
            (500, 400, "codex"),
        ]);
        assert!(match_process_tree(100, &["claude"], &t, 0));
        assert_eq!(find_child_pid(100, "claude", &t, 0), Some(300));
        // codex sits 4 levels below 100 — beyond the depth cap.
        assert!(!match_process_tree(100, &["codex"], &t, 0));
        assert_eq!(find_child_pid(100, "codex", &t, 0), None);
    }

    #[test]
    fn amp_title_parsing() {
        assert_eq!(
            resolve_amp_pane_info("amp - Fix the tests - ~/code/x").as_deref(),
            Some("Fix the tests")
        );
        assert_eq!(resolve_amp_pane_info("amp - just-a-name").as_deref(), Some("just-a-name"));
        assert_eq!(resolve_amp_pane_info("vim"), None);
    }

    fn presence(agent: &str, pane: &str, thread: Option<&str>) -> PaneAgentPresence {
        PaneAgentPresence {
            agent: agent.into(),
            session: "s".into(),
            pane_id: pane.into(),
            thread_id: thread.map(str::to_string),
            thread_name: None,
            status: None,
            last_seen_ts: 1,
        }
    }

    fn pane_map(presences: Vec<PaneAgentPresence>) -> IndexMap<String, PaneAgentPresence> {
        presences.into_iter().map(|p| (format!("{}:pane:{}", p.agent, p.pane_id), p)).collect()
    }

    fn ev(agent: &str, status: AgentStatus, thread: Option<&str>) -> AgentEvent {
        AgentEvent {
            agent: agent.into(),
            session: "s".into(),
            status,
            ts: 1,
            thread_id: thread.map(str::to_string),
            thread_name: None,
            unseen: None,
            pane_id: None,
            details: None,
        }
    }

    #[test]
    fn assemble_scans_only_process_tree_and_resolves_threads() {
        let panes = parse_pane_lines("s|%1|100|zsh|Detect Claude session names\ns|%2|200|zsh|x");
        // Pane %1 runs amp (title mentions claude — must NOT match claude);
        // pane %2 runs claude.
        let t = tree(&[(150, 100, "amp"), (250, 200, "claude")]);
        let resolvers = PaneResolvers {
            claude: &|pid| {
                (pid == 250)
                    .then(|| ("tid-1".to_string(), Some("t".to_string()), Some(AgentStatus::Busy)))
            },
            codex: &|_| None,
        };
        let map = assemble_presences(&panes, &t, &HashSet::new(), &resolvers, 9);
        let agents = &map["s"];
        assert_eq!(agents.len(), 2);
        assert_eq!(agents["amp:pane:%1"].agent, "amp");
        assert_eq!(agents["claude-code:pane:%2"].thread_id.as_deref(), Some("tid-1"));
        assert_eq!(agents["claude-code:pane:%2"].status, Some(AgentStatus::Busy));
        // No claude presence in %1 despite "Claude" in the title.
        assert!(!agents.contains_key("claude-code:pane:%1"));
    }

    #[test]
    fn assemble_excludes_sidebar_panes() {
        let panes = parse_pane_lines("s|%9|100|zsh|agentboard-sidebar");
        let t = tree(&[(150, 100, "claude")]);
        let sidebar: HashSet<String> = ["%9".to_string()].into();
        let resolvers = PaneResolvers { claude: &|_| None, codex: &|_| None };
        assert!(assemble_presences(&panes, &t, &sidebar, &resolvers, 0).is_empty());
    }

    #[test]
    fn sets_differ_detects_changes() {
        let a: PaneAgentMap =
            IndexMap::from([("s".to_string(), pane_map(vec![presence("amp", "%1", None)]))]);
        let same: PaneAgentMap =
            IndexMap::from([("s".to_string(), pane_map(vec![presence("amp", "%1", None)]))]);
        let different: PaneAgentMap =
            IndexMap::from([("s".to_string(), pane_map(vec![presence("amp", "%2", None)]))]);
        assert!(!pane_agent_sets_differ(&a, &same));
        assert!(pane_agent_sets_differ(&a, &different));
        assert!(pane_agent_sets_differ(&a, &IndexMap::new()));
    }

    #[test]
    fn override_rewrites_terminal_to_waiting_when_pane_alive() {
        let pa = pane_map(vec![presence("claude-code", "%1", Some("tid"))]);
        let out = override_terminal_if_pane_alive(
            Some(ev("claude-code", AgentStatus::Complete, Some("tid"))),
            Some(&pa),
        )
        .unwrap();
        assert_eq!(out.status, AgentStatus::Idle);
        // Different thread: untouched.
        let out = override_terminal_if_pane_alive(
            Some(ev("claude-code", AgentStatus::Complete, Some("other"))),
            Some(&pa),
        )
        .unwrap();
        assert_eq!(out.status, AgentStatus::Complete);
    }

    #[test]
    fn merge_upgrades_tracked_terminal_and_attaches_pane() {
        let pa = pane_map(vec![presence("claude-code", "%1", Some("tid"))]);
        let merged = merge_agents_with_pane_presence(
            "s",
            vec![ev("claude-code", AgentStatus::Complete, Some("tid"))],
            Some(&pa),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, AgentStatus::Idle);
        assert_eq!(merged[0].pane_id.as_deref(), Some("%1"));
    }

    #[test]
    fn merge_adds_synthetic_for_untracked_pane_agent() {
        let mut p = presence("codex", "%3", Some("codex-tid"));
        p.status = Some(AgentStatus::Busy);
        let pa = pane_map(vec![p]);
        let merged = merge_agents_with_pane_presence("s", vec![], Some(&pa));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].agent, "codex");
        assert_eq!(merged[0].status, AgentStatus::Busy);
        assert_eq!(merged[0].pane_id.as_deref(), Some("%3"));
    }

    #[test]
    fn merge_skips_threadless_presence_when_agent_already_tracked() {
        let pa = pane_map(vec![presence("amp", "%1", None)]);
        let merged = merge_agents_with_pane_presence(
            "s",
            vec![ev("amp", AgentStatus::Busy, Some("amp-tid"))],
            Some(&pa),
        );
        // No synthetic amp row: the watcher already tracks an amp instance.
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].thread_id.as_deref(), Some("amp-tid"));
    }

    #[test]
    fn merge_drops_orphaned_non_terminal_pane_agents() {
        // A watcher agent that previously got a pane attached, whose pane is
        // now gone, and is non-terminal → dropped.
        let mut orphan = ev("claude-code", AgentStatus::Idle, Some("tid"));
        orphan.pane_id = Some("%dead".into());
        let merged = merge_agents_with_pane_presence("s", vec![orphan.clone()], None);
        assert!(merged.is_empty());
        // Terminal orphans are kept.
        orphan.status = AgentStatus::Complete;
        let merged = merge_agents_with_pane_presence("s", vec![orphan], None);
        assert_eq!(merged.len(), 1);
    }
}
