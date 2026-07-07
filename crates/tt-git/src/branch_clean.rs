//! Merged-branch filtering, ported from `src/commands/gh/branch-clean.ts`.

/// Given the raw stdout of `git branch --merged <base>`, return the branch names that
/// are safe to delete: every merged branch except the protected set
/// (`main`, `master`, `develop`, `dev`, the base branch, and the current branch)
/// and branches that aren't deletion candidates at all (see [`parse_branch_line`]).
pub fn branches_to_delete(merged_stdout: &str, base: &str, current: &str) -> Vec<String> {
    let protected = ["main", "master", "develop", "dev", base, current];
    merged_stdout
        .lines()
        .filter_map(parse_branch_line)
        .filter(|b| !protected.contains(&b.as_str()))
        .collect()
}

/// Parse one `git branch --merged` line into a deletable branch name, or `None`
/// when the line isn't a deletion candidate.
///
/// Git prefixes each line with a two-column marker: `* ` for the branch checked
/// out in *this* worktree, `+ ` for a branch checked out in *another* worktree
/// (git ≥ 2.23), and two spaces otherwise. A branch held by another worktree
/// (`+`) can't be deleted and is someone's active work, so it's excluded. A
/// detached HEAD renders as `* (HEAD detached at <ref>)`; the parenthetical isn't
/// a branch name, so lines whose remainder starts with `(` are excluded too. The
/// current branch (`* <name>`) is returned here and filtered out by the caller's
/// protected set.
fn parse_branch_line(line: &str) -> Option<String> {
    let marker = line.chars().next()?;
    // A branch held by another worktree is never a deletion candidate.
    if marker == '+' {
        return None;
    }
    // Strip only the marker column, not the whole line, so the marker never
    // survives as part of the name (e.g. a bogus `+ branch`).
    let name = match marker {
        '*' => line[1..].trim(),
        _ => line.trim(),
    };
    if name.is_empty() || name.starts_with('(') {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_protected_and_current() {
        let stdout = "  main\n* feature/old\n  develop\n  chore/done\n";
        let result = branches_to_delete(stdout, "main", "feature/current");
        assert_eq!(result, vec!["feature/old".to_string(), "chore/done".to_string()]);
    }

    #[test]
    fn strips_current_branch_marker() {
        // The current branch is marked with `* ` and must be excluded too.
        let stdout = "* feature/current\n  main\n  feature/mergeable\n";
        let result = branches_to_delete(stdout, "main", "feature/current");
        assert_eq!(result, vec!["feature/mergeable".to_string()]);
    }

    #[test]
    fn empty_when_only_protected() {
        let stdout = "  main\n  master\n  dev\n";
        let result = branches_to_delete(stdout, "main", "main");
        assert!(result.is_empty());
    }

    #[test]
    fn respects_custom_base() {
        let stdout = "  release\n  feature/x\n";
        let result = branches_to_delete(stdout, "release", "feature/current");
        assert_eq!(result, vec!["feature/x".to_string()]);
    }

    #[test]
    fn excludes_worktree_checked_out_branch() {
        // `+ ` marks a branch checked out in another worktree: not deletable, and
        // the marker must not leak into the name as a bogus `+ feature/x`.
        let stdout = "  main\n+ feature/in-worktree\n  chore/done\n";
        let result = branches_to_delete(stdout, "main", "feature/current");
        assert_eq!(result, vec!["chore/done".to_string()]);
    }

    #[test]
    fn excludes_detached_head_line() {
        // `* (HEAD detached at <ref>)` is not a branch name.
        let stdout = "* (HEAD detached at abc1234)\n  feature/mergeable\n  main\n";
        let result = branches_to_delete(stdout, "main", "");
        assert_eq!(result, vec!["feature/mergeable".to_string()]);
    }

    #[test]
    fn normal_two_space_branch_is_a_candidate() {
        let stdout = "  feature/y\n";
        let result = branches_to_delete(stdout, "main", "feature/current");
        assert_eq!(result, vec!["feature/y".to_string()]);
    }
}
