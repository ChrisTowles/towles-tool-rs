//! Per-session listening-port attribution. Ports slot-1
//! `runtime/server/port-scanner.ts`.
//!
//! Structure: a pure [`attribute_ports`] (ps-tree BFS + lsof parsing, unit-tested
//! on fixture strings) and a [`PortScanner`] that owns the snapshot and diff. The
//! functions that actually run `tmux`/`ps`/`lsof` are the thin subprocess layer
//! ([`gather_pane_pids`], [`run_ps`], [`run_lsof`]) and are not unit-tested. The
//! `setInterval` port poll (`startPortPoll`/`poll.ts`) is a transport concern and
//! is not ported.

use std::collections::{HashMap, HashSet};

/// Build parent→children PID map from `ps -eo pid=,ppid=` output.
fn parse_ps_children(ps_out: &str) -> HashMap<i64, Vec<i64>> {
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for line in ps_out.lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<i64>(), ppid.parse::<i64>()) else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
    }
    children
}

/// BFS from a session's pane PIDs to its full descendant PID set.
fn descendants(pane_pids: &[i64], children: &HashMap<i64, Vec<i64>>) -> HashSet<i64> {
    let mut all: HashSet<i64> = pane_pids.iter().copied().collect();
    let mut queue: Vec<i64> = pane_pids.to_vec();
    while let Some(pid) = queue.pop() {
        if let Some(kids) = children.get(&pid) {
            for &kid in kids {
                if all.insert(kid) {
                    queue.push(kid);
                }
            }
        }
    }
    all
}

/// Attribute listening TCP ports to sessions.
///
/// Pure port of steps 2–6 of `refreshPortSnapshot`: build the ps tree, BFS each
/// session's descendants, then walk `lsof -F pn` output attributing each
/// listening port to the owning session(s). Every name in `session_names` is
/// present in the result (empty vec when it owns no ports). Ports are sorted
/// ascending. `ps_out` is `ps -eo pid=,ppid=`; `lsof_out` is `lsof -F pn` output.
pub fn attribute_ports(
    pane_pids_by_session: &HashMap<String, Vec<i64>>,
    ps_out: &str,
    lsof_out: &str,
    session_names: &[String],
) -> HashMap<String, Vec<u32>> {
    let children = parse_ps_children(ps_out);

    // pid → sessions owning it (via descendant trees).
    let mut pid_to_sessions: HashMap<i64, Vec<String>> = HashMap::new();
    for (name, pane_pids) in pane_pids_by_session {
        for pid in descendants(pane_pids, &children) {
            pid_to_sessions.entry(pid).or_default().push(name.clone());
        }
    }

    // Walk lsof -F output: `p<pid>` lines set the current pid, `n<host:port>`
    // lines attribute the port to that pid's sessions.
    let mut session_ports: HashMap<String, HashSet<u32>> = HashMap::new();
    let mut current_pid = 0i64;
    for line in lsof_out.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            current_pid = rest.trim().parse::<i64>().unwrap_or(0);
        } else if line.starts_with('n') {
            let Some(sessions) = pid_to_sessions.get(&current_pid) else {
                continue;
            };
            let Some(port) = parse_trailing_port(line) else {
                continue;
            };
            for name in sessions {
                session_ports.entry(name.clone()).or_default().insert(port);
            }
        }
    }

    let mut out: HashMap<String, Vec<u32>> = HashMap::new();
    for name in session_names {
        let mut ports: Vec<u32> =
            session_ports.get(name).map(|s| s.iter().copied().collect()).unwrap_or_default();
        ports.sort_unstable();
        out.insert(name.clone(), ports);
    }
    out
}

/// Extract the trailing `:<port>` from an lsof `n` line (e.g. `n127.0.0.1:8080`).
fn parse_trailing_port(line: &str) -> Option<u32> {
    let colon = line.rfind(':')?;
    let tail = &line[colon + 1..];
    if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    tail.parse::<u32>().ok()
}

/// Owns the current per-session port snapshot and detects changes across scans.
/// Ports the module-global `portSnapshot` + `mapsEqual` as an owned struct.
#[derive(Debug, Default)]
pub struct PortScanner {
    snapshot: HashMap<String, Vec<u32>>,
}

impl PortScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ports for a session (empty when none/unknown). Ports `getSessionPorts`.
    pub fn get(&self, session: &str) -> Vec<u32> {
        self.snapshot.get(session).cloned().unwrap_or_default()
    }

    /// Recompute the snapshot from already-gathered command output and report
    /// whether it changed. The unit-testable core of `refreshPortSnapshot`.
    pub fn refresh_from_outputs(
        &mut self,
        session_names: &[String],
        pane_pids_by_session: &HashMap<String, Vec<i64>>,
        ps_out: &str,
        lsof_out: &str,
    ) -> bool {
        if pane_pids_by_session.is_empty() {
            let changed = !self.snapshot.is_empty();
            self.snapshot = HashMap::new();
            return changed;
        }
        let next = attribute_ports(pane_pids_by_session, ps_out, lsof_out, session_names);
        let changed = next != self.snapshot;
        self.snapshot = next;
        changed
    }

    /// Gather live pane PIDs, ps tree, and lsof, then recompute. The thin
    /// subprocess-driven entry point (not unit-tested). Ports `refreshPortSnapshot`.
    pub fn refresh(&mut self, session_names: &[String]) -> bool {
        let pane_pids = gather_pane_pids(session_names);
        if pane_pids.is_empty() {
            let changed = !self.snapshot.is_empty();
            self.snapshot = HashMap::new();
            return changed;
        }
        let Some(ps_out) = run_ps() else {
            return false;
        };
        let Some(lsof_out) = run_lsof() else {
            return false;
        };
        self.refresh_from_outputs(session_names, &pane_pids, &ps_out, &lsof_out)
    }
}

/// Gather pane PIDs per session via `tmux list-panes`. Thin subprocess layer.
fn gather_pane_pids(session_names: &[String]) -> HashMap<String, Vec<i64>> {
    let mut out: HashMap<String, Vec<i64>> = HashMap::new();
    for name in session_names {
        let Ok(res) = tt_exec::run("tmux", &["list-panes", "-s", "-t", name, "-F", "#{pane_pid}"])
        else {
            continue;
        };
        let pids: Vec<i64> =
            res.stdout.trim().lines().filter_map(|l| l.trim().parse::<i64>().ok()).collect();
        if !pids.is_empty() {
            out.insert(name.clone(), pids);
        }
    }
    out
}

/// Run `ps -eo pid=,ppid=`. Thin subprocess layer.
fn run_ps() -> Option<String> {
    tt_exec::run("ps", &["-eo", "pid=,ppid="]).ok().map(|o| o.stdout)
}

/// Run `lsof -iTCP -sTCP:LISTEN -nP -F pn`. Thin subprocess layer; `None` on failure.
fn run_lsof() -> Option<String> {
    match tt_exec::run("lsof", &["-iTCP", "-sTCP:LISTEN", "-nP", "-F", "pn"]) {
        Ok(out) if out.ok() => Some(out.stdout),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn pane_map(pairs: &[(&str, &[i64])]) -> HashMap<String, Vec<i64>> {
        pairs.iter().map(|(n, pids)| (n.to_string(), pids.to_vec())).collect()
    }

    #[test]
    fn attributes_direct_and_descendant_ports() {
        // pane pid 100 for "web"; 100 → 200 → 300 (a dev server deep in the tree).
        let ps = "100 1\n200 100\n300 200\n999 1\n";
        // lsof: pid 300 listens on 5173, pid 999 (unrelated) on 9999.
        let lsof = "p300\nn127.0.0.1:5173\np999\nn*:9999\n";
        let panes = pane_map(&[("web", &[100])]);
        let result = attribute_ports(&panes, ps, lsof, &names(&["web"]));
        assert_eq!(result["web"], vec![5173]);
    }

    #[test]
    fn ports_sorted_and_deduped() {
        let ps = "100 1\n200 100\n";
        // pane pid 100 and child 200 both listen; ports arrive out of order.
        let lsof = "p200\nn[::1]:8080\np100\nn127.0.0.1:3000\np200\nn127.0.0.1:8080\n";
        let panes = pane_map(&[("s", &[100])]);
        let result = attribute_ports(&panes, ps, lsof, &names(&["s"]));
        assert_eq!(result["s"], vec![3000, 8080]); // sorted, 8080 deduped
    }

    #[test]
    fn session_with_no_ports_present_as_empty() {
        let ps = "100 1\n";
        let lsof = "p999\nn127.0.0.1:1234\n";
        let panes = pane_map(&[("idle", &[100])]);
        let result = attribute_ports(&panes, ps, lsof, &names(&["idle"]));
        assert_eq!(result["idle"], Vec::<u32>::new());
    }

    #[test]
    fn shared_pid_attributes_to_all_owning_sessions() {
        // Both sessions include pid 100 (shared ancestor), which listens on 7000.
        let ps = "100 1\n";
        let lsof = "p100\nn127.0.0.1:7000\n";
        let panes = pane_map(&[("a", &[100]), ("b", &[100])]);
        let result = attribute_ports(&panes, ps, lsof, &names(&["a", "b"]));
        assert_eq!(result["a"], vec![7000]);
        assert_eq!(result["b"], vec![7000]);
    }

    #[test]
    fn scanner_reports_change_then_stable() {
        let mut scanner = PortScanner::new();
        let ps = "100 1\n";
        let lsof = "p100\nn127.0.0.1:8000\n";
        let panes = pane_map(&[("s", &[100])]);
        assert!(scanner.refresh_from_outputs(&names(&["s"]), &panes, ps, lsof));
        assert_eq!(scanner.get("s"), vec![8000]);
        // Same inputs → no change.
        assert!(!scanner.refresh_from_outputs(&names(&["s"]), &panes, ps, lsof));
        // Port disappears → change.
        assert!(scanner.refresh_from_outputs(&names(&["s"]), &panes, ps, "p100\n"));
        assert_eq!(scanner.get("s"), Vec::<u32>::new());
    }

    #[test]
    fn empty_pane_map_clears_snapshot() {
        let mut scanner = PortScanner::new();
        let ps = "100 1\n";
        let lsof = "p100\nn127.0.0.1:8000\n";
        scanner.refresh_from_outputs(&names(&["s"]), &pane_map(&[("s", &[100])]), ps, lsof);
        // No panes now → snapshot cleared, reports change.
        assert!(scanner.refresh_from_outputs(&names(&["s"]), &HashMap::new(), ps, lsof));
        assert_eq!(scanner.get("s"), Vec::<u32>::new());
    }

    #[test]
    fn parse_trailing_port_handles_ipv6_and_junk() {
        assert_eq!(parse_trailing_port("n127.0.0.1:8080"), Some(8080));
        assert_eq!(parse_trailing_port("n[::1]:443"), Some(443));
        assert_eq!(parse_trailing_port("n*:22"), Some(22));
        assert_eq!(parse_trailing_port("nlocalhost:notaport"), None);
        assert_eq!(parse_trailing_port("nnoport"), None);
    }
}
