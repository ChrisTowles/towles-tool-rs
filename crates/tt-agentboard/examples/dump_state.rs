//! Scratch: headless one-shot of the agentboard engine pipeline (scan → track →
//! assemble) to compare against the TS agentboard. Not part of the app.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use tt_agentboard::types::AgentEvent;
use tt_agentboard::{
    AgentTracker, AgentWatcher, AmpAgentWatcher, ClaudeCodeAgentWatcher, ClaudePidLookup,
    CodexAgentWatcher, GitInfoCache, OpenCodeAgentWatcher, RepoEntry, SessionMetadataStore,
    SessionOrder, WatcherContext, assemble_state, default_repos_path, instance_key, load_repos,
    repo_entries, resolve_session_name,
};

struct Ctx {
    entries: Vec<RepoEntry>,
    events: Vec<AgentEvent>,
}

impl WatcherContext for Ctx {
    fn resolve_session(&self, project_dir: &str) -> Option<String> {
        resolve_session_name(project_dir, &self.entries)
    }
    fn emit(&mut self, event: AgentEvent) {
        self.events.push(event);
    }
}

fn main() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;

    let repo_paths = load_repos(&default_repos_path());
    eprintln!("repos.json: {repo_paths:?}");
    let entries = repo_entries(&repo_paths);

    let projects_dir = dirs::home_dir().unwrap().join(".claude").join("projects");
    let mut watchers: Vec<(&str, Box<dyn AgentWatcher>)> = vec![
        (
            "claude-code",
            Box::new(ClaudeCodeAgentWatcher::new(
                projects_dir,
                ClaudePidLookup::new(ClaudePidLookup::default_dir()),
            )),
        ),
        ("amp", Box::new(AmpAgentWatcher::with_defaults())),
        ("codex", Box::new(CodexAgentWatcher::with_defaults())),
        ("opencode", Box::new(OpenCodeAgentWatcher::with_defaults())),
    ];

    let mut ctx = Ctx { entries: entries.clone(), events: Vec::new() };
    for (name, watcher) in &mut watchers {
        let before = ctx.events.len();
        watcher.scan(&mut ctx, now);
        eprintln!("watcher {name}: {} event(s)", ctx.events.len() - before);
    }
    for ev in &ctx.events {
        eprintln!(
            "  event: agent={} session={} status={:?} thread={:?}",
            ev.agent, ev.session, ev.status, ev.thread_id
        );
    }

    let mut tracker = AgentTracker::new();
    for event in ctx.events {
        tracker.apply_event(event, true);
    }

    // pid-liveness (mirrors tt-app compute_payload)
    let mut pid_lookup = ClaudePidLookup::new(ClaudePidLookup::default_dir());
    pid_lookup.invalidate();
    let mut live_threads: HashSet<String> = HashSet::new();
    let mut pinned: HashMap<String, Vec<String>> = HashMap::new();
    for entry in &entries {
        for agent in tracker.get_agents(&entry.name) {
            let Some(tid) = agent.thread_id.clone() else {
                continue;
            };
            let alive = pid_lookup.pid_for_thread(&tid).is_some_and(|p| pid_lookup.is_alive(p));
            eprintln!("  pid-liveness: thread={tid} alive={alive}");
            if alive {
                live_threads.insert(tid.clone());
                pinned
                    .entry(entry.name.clone())
                    .or_default()
                    .push(instance_key(&agent.agent, Some(&tid)));
            }
        }
    }
    tracker.set_pinned_instances_multi(&pinned);
    tracker.prune_stuck(3 * 60 * 1000, now);
    tracker.prune_terminal(now);
    tracker.prune_stale(12 * 60 * 60 * 1000, now);
    tracker.prune_idle(30 * 1000, now);
    tracker.prune_superseded_by_pane();

    let mut git_cache = GitInfoCache::new();
    let mut git_infos = HashMap::new();
    for entry in &entries {
        git_cache.refresh(&entry.dir, now);
        git_infos.insert(entry.dir.clone(), git_cache.get(&entry.dir));
    }

    let metadata = SessionMetadataStore::new();
    let mut order = SessionOrder::new(None);
    let payload = assemble_state(
        &entries,
        &git_infos,
        &tracker,
        &metadata,
        &mut order,
        None,
        "code",
        &live_threads,
        now,
    );
    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
}
