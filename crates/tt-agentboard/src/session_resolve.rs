//! Project-dir → tmux-session resolution for the tmux-mode server. Ports the
//! `watcherCtx.resolveSession` logic from slot-1 `server/index.ts` (the
//! dir-map edition used when sessions come from live tmux sessions rather
//! than repos.json — that edition lives in [`crate::repos`]).
//!
//! Resolution order:
//! 1. exact dir match;
//! 2. most specific (longest) session dir related to the project dir by a
//!    `/`-boundary prefix in either direction — without the specificity rule,
//!    `/a/b` could match session `/a/b/c` before `/a/b/c/d`;
//! 3. encoded fallback: Claude Code encodes project dirs by replacing `/`
//!    with `-`, which is lossy for dash-containing names. Re-encode each
//!    known session dir and match encoded↔encoded prefixes.

/// Encode a path the same way Claude Code does: replace `/` with `-`.
pub fn encode_project_dir(dir: &str) -> String {
    dir.replace('/', "-")
}

/// Resolve `project_dir` against `(dir, session_name)` pairs.
pub fn resolve_session_by_dir(project_dir: &str, dir_map: &[(String, String)]) -> Option<String> {
    // 1. Exact match.
    if let Some((_, name)) = dir_map.iter().find(|(dir, _)| dir == project_dir) {
        return Some(name.clone());
    }

    // 2. Longest related dir on a `/` boundary.
    let mut best: Option<&str> = None;
    let mut best_len = 0;
    for (dir, name) in dir_map {
        let related = project_dir.starts_with(&format!("{dir}/"))
            || dir.starts_with(&format!("{project_dir}/"));
        if related && dir.len() > best_len {
            best_len = dir.len();
            best = Some(name);
        }
    }
    if let Some(name) = best {
        return Some(name.to_string());
    }

    // 3. Encoded fallback (dash ambiguity).
    let encoded = encode_project_dir(project_dir);
    let mut best: Option<&str> = None;
    let mut best_len = 0;
    for (dir, name) in dir_map {
        let encoded_dir = encode_project_dir(dir);
        if (encoded.starts_with(&encoded_dir) || encoded_dir.starts_with(&encoded))
            && encoded_dir.len() > best_len
        {
            best_len = encoded_dir.len();
            best = Some(name);
        }
    }
    best.map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(d, n)| (d.to_string(), n.to_string())).collect()
    }

    #[test]
    fn exact_match_wins() {
        let m = map(&[("/home/u/proj", "proj")]);
        assert_eq!(resolve_session_by_dir("/home/u/proj", &m).as_deref(), Some("proj"));
    }

    #[test]
    fn longest_related_dir_wins() {
        // /a/b would match both sessions; the deeper dir must win.
        let m = map(&[("/a/b/c", "shallow"), ("/a/b/c/d", "deep")]);
        assert_eq!(resolve_session_by_dir("/a/b/c/d/e", &m).as_deref(), Some("deep"));
        // Session dir deeper than the project dir also relates (worktree parent).
        assert_eq!(resolve_session_by_dir("/a/b", &m).as_deref(), Some("deep"));
    }

    #[test]
    fn prefix_requires_slash_boundary() {
        let m = map(&[("/home/u/proj", "proj")]);
        // /home/u/proj-extra is NOT under /home/u/proj (plain-prefix trap)...
        // but the encoded fallback intentionally matches it, faithful to the
        // TS: encoded forms cannot distinguish `-` from `/`.
        assert_eq!(resolve_session_by_dir("/home/u/proj-extra", &m).as_deref(), Some("proj"));
        // A genuinely unrelated dir resolves to nothing.
        assert_eq!(resolve_session_by_dir("/home/u/other", &m), None);
    }

    #[test]
    fn encoded_fallback_handles_dash_ambiguity() {
        // Claude Code hands the watcher `-home-u-my-app` decoded naively as
        // `/home/u/my/app`, which matches no real dir — but encodes equal.
        let m = map(&[("/home/u/my-app", "my-app")]);
        assert_eq!(resolve_session_by_dir("/home/u/my/app", &m).as_deref(), Some("my-app"));
    }

    #[test]
    fn empty_map_resolves_nothing() {
        assert_eq!(resolve_session_by_dir("/a", &[]), None);
    }
}
