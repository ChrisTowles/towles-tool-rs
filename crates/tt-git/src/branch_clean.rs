//! Merged-branch filtering, ported from `src/commands/gh/branch-clean.ts`.

/// Given the raw stdout of `git branch --merged <base>`, return the branch names that
/// are safe to delete: every merged branch except the protected set
/// (`main`, `master`, `develop`, `dev`, the base branch, and the current branch).
///
/// Mirrors the TS: each line is trimmed, a leading `* ` (current-branch marker) is
/// stripped, empty lines are dropped, then protected branches are filtered out.
pub fn branches_to_delete(merged_stdout: &str, base: &str, current: &str) -> Vec<String> {
    let protected = ["main", "master", "develop", "dev", base, current];
    merged_stdout
        .split('\n')
        .map(|line| line.trim().strip_prefix("* ").unwrap_or(line.trim()).to_string())
        .filter(|b| !b.is_empty())
        .filter(|b| !protected.contains(&b.as_str()))
        .collect()
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
}
