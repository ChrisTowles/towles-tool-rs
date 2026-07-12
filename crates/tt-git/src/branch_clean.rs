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

/// Given the raw stdout of `git branch -vv`, return the branch names whose
/// upstream is `gone` (the remote branch was deleted — e.g. after a GitHub
/// rebase-and-merge, which lands the commits under *new* SHAs so `git branch
/// --merged` never lists them). The same protected set as [`branches_to_delete`]
/// is excluded (`main`, `master`, `develop`, `dev`, the base, the current
/// branch). These branches aren't ancestor-merged, so the caller deletes them
/// with `git branch -D`.
pub fn branches_gone(vv_stdout: &str, base: &str, current: &str) -> Vec<String> {
    let protected = ["main", "master", "develop", "dev", base, current];
    vv_stdout
        .lines()
        .filter_map(parse_gone_branch_line)
        .filter(|b| !protected.contains(&b.as_str()))
        .collect()
}

/// Parse one `git branch -vv` line into a branch name whose upstream is `gone`,
/// or `None` otherwise.
///
/// A verbose line looks like `  feat/x  abc1234 [origin/feat/x: gone] subject`.
/// The two-column marker is handled like [`parse_branch_line`]: `+` (another
/// worktree) and detached-HEAD parentheticals are skipped. After the branch
/// name comes the short SHA, then an optional `[<upstream>: <status>]` tracking
/// bracket; the upstream is `gone` exactly when that bracket's contents end in
/// `: gone`. Matching the parsed bracket (not a raw substring) keeps a commit
/// subject that happens to contain `: gone` from being mistaken for a match.
fn parse_gone_branch_line(line: &str) -> Option<String> {
    let marker = line.chars().next()?;
    // A branch held by another worktree is never a deletion candidate.
    if marker == '+' {
        return None;
    }
    let rest = match marker {
        '*' => line[1..].trim(),
        _ => line.trim(),
    };
    let (name, remainder) = rest.split_once(char::is_whitespace)?;
    if name.is_empty() || name.starts_with('(') {
        return None;
    }
    let start = remainder.find('[')?;
    let end = remainder[start..].find(']')? + start;
    let tracking = &remainder[start + 1..end];
    if tracking.ends_with(": gone") { Some(name.to_string()) } else { None }
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

    #[test]
    fn gone_picks_only_gone_upstreams() {
        let stdout = "\
* main       abc1234 [origin/main] latest
  feat/done  def5678 [origin/feat/done: gone] finished work
  feat/wip   0011223 [origin/feat/wip: ahead 2] in progress
  local-only 4455667 no upstream at all
";
        let result = branches_gone(stdout, "main", "main");
        assert_eq!(result, vec!["feat/done".to_string()]);
    }

    #[test]
    fn gone_excludes_protected_and_current() {
        let stdout = "\
* feat/current 1111111 [origin/feat/current: gone] on this branch
  develop      2222222 [origin/develop: gone] protected name
  chore/old    3333333 [origin/chore/old: gone] cleanup
";
        let result = branches_gone(stdout, "main", "feat/current");
        assert_eq!(result, vec!["chore/old".to_string()]);
    }

    #[test]
    fn gone_ignores_subject_containing_gone_marker() {
        // The tracking bracket is present and not gone; the subject text must
        // not trigger a false positive.
        let stdout = "  feat/x 9988776 [origin/feat/x] revert \"branch: gone\" flag\n";
        let result = branches_gone(stdout, "main", "feat/current");
        assert!(result.is_empty());
    }

    #[test]
    fn gone_excludes_worktree_and_detached_lines() {
        let stdout = "\
+ feat/in-worktree aaa1111 [origin/feat/in-worktree: gone] held elsewhere
* (HEAD detached at bbb2222) some subject
  feat/real        ccc3333 [origin/feat/real: gone] deletable
";
        let result = branches_gone(stdout, "main", "");
        assert_eq!(result, vec!["feat/real".to_string()]);
    }
}
