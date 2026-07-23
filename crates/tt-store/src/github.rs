//! GitHub caches: the issues and PR tables (full-swap and state-scoped
//! writes), the tracked-repo identity cache, and their dismissal-aware reads.

use rusqlite::params;

use crate::model::*;
use crate::{Error, Result, Store};

impl Store {
    /// Replace only the named repos' issue rows, leaving other repos' rows
    /// intact. Collectors use this when a sweep partially failed: repos that
    /// errored keep their last-known-good rows instead of being wiped.
    pub fn replace_issues_for_repos(
        &self,
        repos: &[String],
        issues: &[IssueInput],
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut del = tx.prepare("DELETE FROM issues WHERE repo = ?1")?;
            for repo in repos {
                del.execute(params![repo])?;
            }
            let mut stmt = tx.prepare(
                "INSERT INTO issues (repo, number, title, labels, state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for i in issues {
                stmt.execute(params![
                    i.repo,
                    i.number,
                    i.title,
                    serde_json::to_string(&i.labels)?,
                    i.state,
                    i.url,
                    i.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(issues.len())
    }

    /// Full-snapshot replace of issue rows.
    pub fn replace_issues(&self, issues: &[IssueInput]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM issues", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO issues (repo, number, title, labels, state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for i in issues {
                stmt.execute(params![
                    i.repo,
                    i.number,
                    i.title,
                    serde_json::to_string(&i.labels)?,
                    i.state,
                    i.url,
                    i.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(issues.len())
    }

    /// Reconcile the tracked-repo identity cache to exactly `repos`
    /// (`repo_root` -> `owner_repo` pairs): upsert each pair, then delete any
    /// existing row whose `repo_root` isn't in the set. The Agentboard poll
    /// loop calls this every cycle with the currently tracked repos and their
    /// freshly-derived git origin, so untracking a repo (or its origin
    /// becoming unparseable) drops its row on the next poll with no separate
    /// untrack step — `repos.json` stays the one source of truth for which
    /// repos exist, and this table can never drift into holding a stale one.
    pub fn reconcile_repos(&self, repos: &[(String, String)], now_ms: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut upsert = tx.prepare(
                "INSERT INTO repos (repo_root, owner_repo, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(repo_root) DO UPDATE SET owner_repo = excluded.owner_repo,
                                                       updated_at = excluded.updated_at",
            )?;
            for (repo_root, owner_repo) in repos {
                upsert.execute(params![repo_root, owner_repo, now_ms])?;
            }
            if repos.is_empty() {
                tx.execute("DELETE FROM repos", [])?;
            } else {
                let placeholders = repos.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                let mut del = tx.prepare(&format!(
                    "DELETE FROM repos WHERE repo_root NOT IN ({placeholders})"
                ))?;
                let roots: Vec<&String> = repos.iter().map(|(root, _)| root).collect();
                del.execute(rusqlite::params_from_iter(roots))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The tracked repo root for a given `owner/repo` slug, if the identity
    /// cache currently knows it. `task_create` validates its `repo` argument
    /// against this instead of matching a dir/basename.
    pub fn repo_root_for_owner_repo(&self, owner_repo: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT repo_root FROM repos WHERE owner_repo = ?1",
                params![owner_repo],
                |r| r.get(0),
            )
            .optional()
            .map_err(Error::from)
    }

    /// Every tracked repo's `owner/repo` slug, sorted for a stable error
    /// message when `task_create` rejects an unknown `repo` argument.
    pub fn repo_slugs(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT owner_repo FROM repos ORDER BY owner_repo")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }

    /// Replace only the named repos' PR rows, leaving other repos' rows intact.
    /// See [`Store::replace_issues_for_repos`] for the failure-containment
    /// rationale.
    pub fn replace_prs_for_repos(&self, repos: &[String], prs: &[PrInput]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut del = tx.prepare("DELETE FROM pr_status WHERE repo = ?1")?;
            for repo in repos {
                del.execute(params![repo])?;
            }
            let mut stmt = tx.prepare(
                "INSERT INTO pr_status
                   (repo, number, title, branch, state, checks, review_state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for p in prs {
                stmt.execute(params![
                    p.repo,
                    p.number,
                    p.title,
                    p.branch,
                    p.state,
                    p.checks,
                    p.review_state,
                    p.url,
                    p.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(prs.len())
    }

    /// Full-snapshot replace of PR status rows.
    pub fn replace_prs(&self, prs: &[PrInput]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM pr_status", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO pr_status
                   (repo, number, title, branch, state, checks, review_state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for p in prs {
                stmt.execute(params![
                    p.repo,
                    p.number,
                    p.title,
                    p.branch,
                    p.state,
                    p.checks,
                    p.review_state,
                    p.url,
                    p.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(prs.len())
    }

    /// Replace only the non-merged PR rows for `repos`, leaving each repo's
    /// merged rows and every other repo's rows intact. Used by the fast,
    /// frequent open-PR sweep so it never has to re-fetch (and thus never
    /// clobbers) the separately-cadenced merged-PR rows — see
    /// [`Store::replace_merged_prs_for_repos`].
    pub fn replace_open_prs_for_repos(&self, repos: &[String], prs: &[PrInput]) -> Result<usize> {
        self.replace_prs_for_repos_where(repos, prs, "state != 'merged'")
    }

    /// Full-snapshot replace of the non-merged PR rows, preserving merged rows.
    pub fn replace_open_prs(&self, prs: &[PrInput]) -> Result<usize> {
        self.replace_prs_where(prs, "state != 'merged'")
    }

    /// Replace only the merged PR rows for `repos`, leaving each repo's open
    /// rows intact. See [`Store::replace_open_prs_for_repos`].
    pub fn replace_merged_prs_for_repos(&self, repos: &[String], prs: &[PrInput]) -> Result<usize> {
        self.replace_prs_for_repos_where(repos, prs, "state = 'merged'")
    }

    /// Full-snapshot replace of the merged PR rows, preserving open rows.
    pub fn replace_merged_prs(&self, prs: &[PrInput]) -> Result<usize> {
        self.replace_prs_where(prs, "state = 'merged'")
    }

    fn replace_prs_for_repos_where(
        &self,
        repos: &[String],
        prs: &[PrInput],
        state_predicate: &str,
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut del = tx
                .prepare(&format!("DELETE FROM pr_status WHERE repo = ?1 AND {state_predicate}"))?;
            for repo in repos {
                del.execute(params![repo])?;
            }
            let mut stmt = tx.prepare(
                "INSERT INTO pr_status
                   (repo, number, title, branch, state, checks, review_state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for p in prs {
                stmt.execute(params![
                    p.repo,
                    p.number,
                    p.title,
                    p.branch,
                    p.state,
                    p.checks,
                    p.review_state,
                    p.url,
                    p.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(prs.len())
    }

    fn replace_prs_where(&self, prs: &[PrInput], state_predicate: &str) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(&format!("DELETE FROM pr_status WHERE {state_predicate}"), [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO pr_status
                   (repo, number, title, branch, state, checks, review_state, url, updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for p in prs {
                stmt.execute(params![
                    p.repo,
                    p.number,
                    p.title,
                    p.branch,
                    p.state,
                    p.checks,
                    p.review_state,
                    p.url,
                    p.updated_ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(prs.len())
    }

    /// All issue rows, newest update first.
    pub fn issues(&self) -> Result<Vec<IssueItem>> {
        self.query_issues(
            &format!(
                "SELECT {ISSUE_COLS} FROM issues i \
                 LEFT JOIN item_dismissals d \
                   ON d.kind = 'issue' AND d.repo = i.repo AND d.number = i.number \
                 ORDER BY i.updated_ts DESC"
            ),
            [],
        )
    }

    /// A single cached issue row by `(repo, number)`, if the collector has seen it.
    pub fn get_issue(&self, repo: &str, number: i64) -> Result<Option<IssueItem>> {
        Ok(self
            .query_issues(
                &format!(
                    "SELECT {ISSUE_COLS} FROM issues i \
                     LEFT JOIN item_dismissals d \
                       ON d.kind = 'issue' AND d.repo = i.repo AND d.number = i.number \
                     WHERE i.repo = ?1 AND i.number = ?2"
                ),
                params![repo, number],
            )?
            .into_iter()
            .next())
    }

    /// All PR status rows, newest update first.
    pub fn prs(&self) -> Result<Vec<PrItem>> {
        self.query_prs(
            &format!(
                "SELECT {PR_COLS} FROM pr_status p \
                 LEFT JOIN item_dismissals d \
                   ON d.kind = 'pr' AND d.repo = p.repo AND d.number = p.number \
                 ORDER BY p.updated_ts DESC"
            ),
            [],
        )
    }

    fn query_issues(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<IssueItem>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (repo, number, title, labels_json, state, url, updated_ts, dismissed_ts) = row?;
            let labels: Vec<String> = serde_json::from_str(&labels_json)?;
            out.push(IssueItem {
                repo,
                number,
                title,
                labels,
                state,
                url,
                updated_ts,
                dismissed_ts,
            });
        }
        Ok(out)
    }

    fn query_prs(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<PrItem>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(PrItem {
                repo: r.get(0)?,
                number: r.get(1)?,
                title: r.get(2)?,
                branch: r.get(3)?,
                state: r.get(4)?,
                checks: r.get(5)?,
                review_state: r.get(6)?,
                url: r.get(7)?,
                updated_ts: r.get(8)?,
                dismissed_ts: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
