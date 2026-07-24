//! Kanban tasks and their GitHub links: create/move/close/archive, the
//! issue/PR link tables, and the worktree binding (#339's unit of work).

use rusqlite::params;

use crate::model::*;
use crate::{Error, Result, Store};

impl Store {
    /// Add a task. Lands in `status` (validated against [`TASK_STATUSES`]) at
    /// the end of that column; `notes` is free-form context. Issues/PRs are
    /// attached separately ([`Store::attach_task_issue`] /
    /// [`Store::attach_task_pr`]), the worktree via [`Store::set_task_worktree`].
    pub fn add_task(
        &self,
        text: &str,
        status: &str,
        notes: Option<&str>,
        goal: Option<&str>,
        now_ms: i64,
    ) -> Result<TaskItem> {
        if !TASK_STATUSES.contains(&status) {
            return Err(Error::Sqlite(rusqlite::Error::InvalidParameterName(format!(
                "unknown task status: {status}"
            ))));
        }
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE status = ?1",
            params![status],
            |r| r.get(0),
        )?;
        let completed_at: Option<i64> = if status == "done" { Some(now_ms) } else { None };
        self.conn.execute(
            "INSERT INTO tasks (text, status, position, notes, goal, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![text, status, position, notes, goal, now_ms, completed_at],
        )?;
        self.task_by_id(self.conn.last_insert_rowid())
    }

    /// Move a todo to a kanban column, appending it at the end of the target
    /// column (position = max there + 1, ignoring the task itself). Sets
    /// `completed_at` when entering `done`, clears it otherwise. Moving to any
    /// non-`done` column also reopens a closed task — `outcome` and
    /// `archived_at` clear, since the card is active again. Unknown statuses
    /// are rejected.
    pub fn set_task_status(&self, id: i64, status: &str, now_ms: i64) -> Result<()> {
        if !TASK_STATUSES.contains(&status) {
            return Err(Error::Sqlite(rusqlite::Error::InvalidParameterName(format!(
                "unknown task status: {status}"
            ))));
        }
        let completed_at: Option<i64> = if status == "done" { Some(now_ms) } else { None };
        let tx = self.conn.unchecked_transaction()?;
        let position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE status = ?1 AND id <> ?2",
            params![status, id],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2, position = ?3,
                    outcome = CASE WHEN ?1 = 'done' THEN outcome ELSE NULL END,
                    archived_at = CASE WHEN ?1 = 'done' THEN archived_at ELSE NULL END
             WHERE id = ?4",
            params![status, completed_at, position, id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Move a todo to `status` at an explicit `index` within that column,
    /// renumbering the column's `position`s to be contiguous (`0..n`). `index`
    /// is clamped to `[0, n]`, where `n` is the number of *other* todos already
    /// in the column, so out-of-range values land the card at the top or
    /// bottom rather than erroring. Sets `completed_at` when entering `done`
    /// and clears it otherwise (matching [`Store::set_task_status`]).
    ///
    /// Unlike `set_task_status` (which always appends), this reaches an
    /// arbitrary position — it powers drag-to-reorder within a column and
    /// position-aware drops across columns. The source column is left with a
    /// gap in its `position`s, which is harmless: ordering is by relative
    /// `position ASC`, and the next reorder there renumbers it. Returns
    /// [`Error::TaskNotFound`] when no todo has `id`.
    pub fn set_task_position(&self, id: i64, status: &str, index: i64, now_ms: i64) -> Result<()> {
        if !TASK_STATUSES.contains(&status) {
            return Err(Error::Sqlite(rusqlite::Error::InvalidParameterName(format!(
                "unknown task status: {status}"
            ))));
        }
        let tx = self.conn.unchecked_transaction()?;
        // The target column's todos in board order, excluding the mover.
        let others: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM tasks WHERE status = ?1 AND id <> ?2
                 ORDER BY position ASC, created_at ASC",
            )?;
            let rows = stmt.query_map(params![status, id], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()?
        };
        let pos = index.clamp(0, others.len() as i64) as usize;
        let mut order = others;
        order.insert(pos, id);
        {
            let mut up = tx.prepare("UPDATE tasks SET position = ?1 WHERE id = ?2")?;
            for (pos, tid) in order.iter().enumerate() {
                up.execute(params![pos as i64, tid])?;
            }
        }
        let completed_at: Option<i64> = if status == "done" { Some(now_ms) } else { None };
        let affected = tx.execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2,
                    outcome = CASE WHEN ?1 = 'done' THEN outcome ELSE NULL END,
                    archived_at = CASE WHEN ?1 = 'done' THEN archived_at ELSE NULL END
             WHERE id = ?3",
            params![status, completed_at, id],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        tx.commit()?;
        Ok(())
    }

    /// Edit a todo's free-form fields: its `text` and optional `notes`. This
    /// is a full replace of both fields — passing `None` for `notes` clears it
    /// (there is no "leave unchanged" sentinel). Status, position, and any
    /// issue link are left untouched. Returns the updated todo, or
    /// [`Error::TaskNotFound`] when no todo has `id`.
    pub fn update_task(&self, id: i64, text: &str, notes: Option<&str>) -> Result<TaskItem> {
        let affected = self.conn.execute(
            "UPDATE tasks SET text = ?1, notes = ?2 WHERE id = ?3",
            params![text, notes, id],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        self.task_by_id(id)
    }

    /// Delete a task permanently, cascading its issue/PR link rows. Returns
    /// [`Error::TaskNotFound`] when no task has `id`.
    pub fn delete_task(&self, id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let affected = tx.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        tx.execute("DELETE FROM task_issues WHERE task_id = ?1", params![id])?;
        tx.execute("DELETE FROM task_prs WHERE task_id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Close a task: record how it ended and detach it from its worktree
    /// directory — the row survives as the record, this is what replaced
    /// deleting it. Closing as [`TaskOutcome::Done`] also lands the card at
    /// the end of the `done` column (matching [`Store::set_task_status`]);
    /// closing as [`TaskOutcome::Abandoned`] freezes `status` where the work
    /// stopped. Either way `completed_at` is stamped if not already set,
    /// which is what later ages the row into the archive
    /// ([`Store::archive_closed_tasks`]). Returns the updated task, or
    /// [`Error::TaskNotFound`] when no task has `id`.
    pub fn close_task(&self, id: i64, outcome: TaskOutcome, now_ms: i64) -> Result<TaskItem> {
        let outcome = outcome.as_str();
        let tx = self.conn.unchecked_transaction()?;
        let affected = if outcome == "done" {
            let position: i64 = tx.query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks
                 WHERE status = 'done' AND id <> ?1",
                params![id],
                |r| r.get(0),
            )?;
            tx.execute(
                "UPDATE tasks SET status = 'done', position = ?2,
                        completed_at = COALESCE(completed_at, ?3),
                        outcome = ?4, worktree_dir = NULL
                 WHERE id = ?1",
                params![id, position, now_ms, outcome],
            )?
        } else {
            tx.execute(
                "UPDATE tasks SET completed_at = COALESCE(completed_at, ?2),
                        outcome = ?3, worktree_dir = NULL
                 WHERE id = ?1",
                params![id, now_ms, outcome],
            )?
        };
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        tx.commit()?;
        self.task_by_id(id)
    }

    /// Archive one task off the active board now. Archiving twice keeps the
    /// original timestamp. Returns [`Error::TaskNotFound`] when no task has
    /// `id`.
    pub fn archive_task(&self, id: i64, now_ms: i64) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE tasks SET archived_at = COALESCE(archived_at, ?2) WHERE id = ?1",
            params![id, now_ms],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        Ok(())
    }

    /// Bring an archived task back onto the board. Its `status` and `outcome`
    /// are left as they were — it reappears in the terminal column, and a
    /// status move out of there reopens it fully. Returns
    /// [`Error::TaskNotFound`] when no task has `id`.
    pub fn unarchive_task(&self, id: i64) -> Result<()> {
        let affected =
            self.conn.execute("UPDATE tasks SET archived_at = NULL WHERE id = ?1", params![id])?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        Ok(())
    }

    /// Archive closed tasks (an `outcome` on record, or sitting in `done`)
    /// that finished before `before_ms`, returning how many were archived.
    /// This replaced the old hard-delete sweep — history is hidden, never
    /// destroyed. Open tasks and recently-finished ones are left untouched; a
    /// closed row with a NULL `completed_at` (legacy data) is never swept,
    /// since its completion time is unknown. Both instants are injected — the
    /// clock read happens at the call boundary, not here.
    pub fn archive_closed_tasks(&self, before_ms: i64, now_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE tasks SET archived_at = ?2
             WHERE archived_at IS NULL
               AND (outcome IS NOT NULL OR status = 'done')
               AND completed_at IS NOT NULL AND completed_at < ?1",
            params![before_ms, now_ms],
        )?)
    }

    /// Attach a GitHub issue to a task. Re-attaching an existing link only
    /// refreshes the `url` — the cached `state` is preserved (the collector
    /// owns it). Returns [`Error::TaskNotFound`] when no task has `id`.
    pub fn attach_task_issue(&self, id: i64, repo: &str, number: i64, url: &str) -> Result<()> {
        self.require_task(id)?;
        self.conn.execute(
            "INSERT INTO task_issues (task_id, repo, number, url) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(task_id, repo, number) DO UPDATE SET url = excluded.url",
            params![id, repo, number, url],
        )?;
        Ok(())
    }

    /// Detach a GitHub issue from a task. Detaching a link that doesn't exist
    /// is a no-op.
    pub fn detach_task_issue(&self, id: i64, repo: &str, number: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM task_issues WHERE task_id = ?1 AND repo = ?2 AND number = ?3",
            params![id, repo, number],
        )?;
        Ok(())
    }

    /// Attach a GitHub PR to a task. Re-attaching refreshes only the `url`
    /// (state/checks stay collector-owned). Returns [`Error::TaskNotFound`]
    /// when no task has `id`.
    pub fn attach_task_pr(&self, id: i64, repo: &str, number: i64, url: &str) -> Result<()> {
        self.require_task(id)?;
        self.conn.execute(
            "INSERT INTO task_prs (task_id, repo, number, url) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(task_id, repo, number) DO UPDATE SET url = excluded.url",
            params![id, repo, number, url],
        )?;
        Ok(())
    }

    /// Detach a GitHub PR from a task. Detaching a link that doesn't exist is
    /// a no-op.
    pub fn detach_task_pr(&self, id: i64, repo: &str, number: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM task_prs WHERE task_id = ?1 AND repo = ?2 AND number = ?3",
            params![id, repo, number],
        )?;
        Ok(())
    }

    /// Bind a task to its repo, and to the worktree its work happens in
    /// once one exists. Called twice in the Agentboard's new-task flow: at
    /// submit with the repo alone (`branch`/`dir` `None`), then again once
    /// `task_create` resolves. A "task only" submit stops after the first.
    ///
    /// The optional columns are upserts, never clears: a `None` means "leave
    /// as is" (`COALESCE`), so a repo-only rebind — e.g. retrying a failed
    /// `task_create` on a task whose worktree already exists — can't erase an
    /// established branch/dir. Nothing here un-sets a branch or a dir: the
    /// one legitimate detach is [`Store::close_task`], which clears `dir` (the
    /// worktree is off disk) while `repo_root`/`branch` survive as historical
    /// fact. Returns [`Error::TaskNotFound`] when no task has `id`.
    pub fn set_task_worktree(
        &self,
        id: i64,
        repo_root: &str,
        repo: Option<&str>,
        branch: Option<&str>,
        dir: Option<&str>,
    ) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE tasks SET worktree_repo_root = ?1,
                              worktree_repo = COALESCE(?2, worktree_repo),
                              worktree_branch = COALESCE(?3, worktree_branch),
                              worktree_dir = COALESCE(?4, worktree_dir)
             WHERE id = ?5",
            params![repo_root, repo, branch, dir, id],
        )?;
        if affected == 0 {
            return Err(Error::TaskNotFound(id));
        }
        Ok(())
    }

    /// Open todos in kanban order: not in `done`, not closed with an
    /// `outcome`, not archived.
    pub fn open_tasks(&self) -> Result<Vec<TaskItem>> {
        self.query_tasks(
            &format!(
                "SELECT {TASK_COLS} FROM tasks
                 WHERE status != 'done' AND outcome IS NULL AND archived_at IS NULL {TASK_ORDER}"
            ),
            [],
        )
    }

    /// A single todo by id, if it exists.
    pub fn get_task(&self, id: i64) -> Result<Option<TaskItem>> {
        Ok(self
            .query_tasks(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"), [id])?
            .into_iter()
            .next())
    }

    /// All tasks in kanban order, links and worktree included. The collectors'
    /// rollup walks this; the board gets it via [`Store::snapshot`].
    pub fn all_tasks(&self) -> Result<Vec<TaskItem>> {
        self.query_tasks(&format!("SELECT {TASK_COLS} FROM tasks {TASK_ORDER}"), [])
    }

    /// Distinct `(repo, number)` PR refs linked to any task.
    pub fn linked_pr_refs(&self) -> Result<Vec<(String, i64)>> {
        self.query_refs("SELECT DISTINCT repo, number FROM task_prs ORDER BY repo, number")
    }

    /// Issue refs whose cached link state is still `open` but which are
    /// missing from the collector's `issues` snapshot — the ambiguous set
    /// (closed? reassigned away?) that needs a targeted `gh issue view`.
    /// Terminal-state links absent from the snapshot are *not* returned:
    /// their cached state stands until the ref reappears in the snapshot.
    pub fn open_issue_refs_missing_from_cache(&self) -> Result<Vec<(String, i64)>> {
        self.query_refs(
            "SELECT DISTINCT ti.repo, ti.number FROM task_issues ti
             WHERE ti.state = 'open'
               AND NOT EXISTS (SELECT 1 FROM issues i
                               WHERE i.repo = ti.repo AND i.number = ti.number)
             ORDER BY ti.repo, ti.number",
        )
    }

    /// PR refs whose cached link state is still `open` but which are missing
    /// from the `pr_status` snapshot. See
    /// [`Store::open_issue_refs_missing_from_cache`].
    pub fn open_pr_refs_missing_from_cache(&self) -> Result<Vec<(String, i64)>> {
        self.query_refs(
            "SELECT DISTINCT tp.repo, tp.number FROM task_prs tp
             WHERE tp.state = 'open'
               AND NOT EXISTS (SELECT 1 FROM pr_status p
                               WHERE p.repo = tp.repo AND p.number = tp.number)
             ORDER BY tp.repo, tp.number",
        )
    }

    /// Stamp the observed state onto every link row for one issue ref.
    pub fn set_issue_link_state(
        &self,
        repo: &str,
        number: i64,
        state: &str,
        now_ms: i64,
    ) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE task_issues SET state = ?3, state_ts = ?4
             WHERE repo = ?1 AND number = ?2",
            params![repo, number, state, now_ms],
        )?)
    }

    /// Stamp the observed state onto every link row for one PR ref. `checks`
    /// updates when given; `None` keeps the cached value (the targeted fetch
    /// only learns the state).
    pub fn set_pr_link_state(
        &self,
        repo: &str,
        number: i64,
        state: &str,
        checks: Option<&str>,
        now_ms: i64,
    ) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE task_prs SET state = ?3, checks = COALESCE(?4, checks), state_ts = ?5
             WHERE repo = ?1 AND number = ?2",
            params![repo, number, state, checks, now_ms],
        )?)
    }

    /// Refresh every issue/PR link row whose ref is present in the collector
    /// snapshot (`issues` / `pr_status`), copying state (and checks) across.
    /// Refs absent from the snapshot are left untouched — see the targeted
    /// fetch in `tt-collect` for those.
    pub fn refresh_link_states_from_cache(&self, now_ms: i64) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let issues = tx.execute(
            "UPDATE task_issues SET
               state = (SELECT i.state FROM issues i
                        WHERE i.repo = task_issues.repo AND i.number = task_issues.number),
               state_ts = ?1
             WHERE EXISTS (SELECT 1 FROM issues i
                           WHERE i.repo = task_issues.repo AND i.number = task_issues.number)",
            params![now_ms],
        )?;
        let prs = tx.execute(
            "UPDATE task_prs SET
               state = (SELECT p.state FROM pr_status p
                        WHERE p.repo = task_prs.repo AND p.number = task_prs.number),
               checks = (SELECT p.checks FROM pr_status p
                         WHERE p.repo = task_prs.repo AND p.number = task_prs.number),
               state_ts = ?1
             WHERE EXISTS (SELECT 1 FROM pr_status p
                           WHERE p.repo = task_prs.repo AND p.number = task_prs.number)",
            params![now_ms],
        )?;
        tx.commit()?;
        Ok(issues + prs)
    }

    /// Auto-attach collected PRs to worktree-bound tasks: any `pr_status` row
    /// whose `(repo, branch)` matches a task's `(worktree_repo, worktree_branch)`
    /// becomes a `task_prs` link — "PRs open in the worktree, linked to the task"
    /// without a manual step. Existing links are left untouched. Archived
    /// tasks are excluded: their kept `branch` is historical fact, and a
    /// reused branch name must not link a future PR to a long-dead task. A
    /// merely *closed* task still attaches — a PR that merges right as the
    /// worktree is deleted completes the record. Returns how many links were
    /// created.
    pub fn auto_attach_worktree_prs(&self, now_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO task_prs (task_id, repo, number, url, state, checks, state_ts)
             SELECT t.id, p.repo, p.number, p.url, p.state, p.checks, ?1
             FROM tasks t
             JOIN pr_status p ON p.repo = t.worktree_repo AND p.branch = t.worktree_branch
             WHERE t.worktree_repo IS NOT NULL AND t.worktree_branch IS NOT NULL
               AND t.archived_at IS NULL",
            params![now_ms],
        )?)
    }

    /// The task bound to the worktree at `dir`, if any (a worktree belongs to at
    /// most one task; if data ever disagrees, the oldest task wins).
    pub fn task_for_worktree_dir(&self, dir: &str) -> Result<Option<TaskItem>> {
        Ok(self
            .query_tasks(
                &format!(
                    "SELECT {TASK_COLS} FROM tasks WHERE worktree_dir = ?1
                     ORDER BY created_at ASC LIMIT 1"
                ),
                params![dir],
            )?
            .into_iter()
            .next())
    }

    // --- Row-mapping helpers ---------------------------------------------

    /// One task by id, with its links and worktree binding (the same row shape
    /// [`Store::open_tasks`] returns).
    pub fn task_by_id(&self, id: i64) -> Result<TaskItem> {
        self.query_tasks(&format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"), [id])?
            // `TaskNotFound`, like every other id lookup in this module — not a
            // fabricated `Sqlite(QueryReturnedNoRows)`. A caller has to be able
            // to tell "this row does not exist" from "the database could not
            // answer", and the `?` above already carries the genuine failures.
            .into_iter()
            .next()
            .ok_or(Error::TaskNotFound(id))
    }

    fn query_tasks(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<TaskItem>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            let worktree_repo_root: Option<String> = r.get(7)?;
            let worktree_repo: Option<String> = r.get(8)?;
            let worktree_branch: Option<String> = r.get(9)?;
            let worktree_dir: Option<String> = r.get(10)?;
            let outcome: Option<String> = r.get(11)?;
            let archived_at: Option<i64> = r.get(12)?;
            let goal: Option<String> = r.get(13)?;
            // Keyed on `repo_root` alone: a repo-bound task with no worktree
            // yet still has a worktree binding, and dropping it here would hide
            // the task's repo from the Board's swimlanes.
            let worktree = worktree_repo_root.map(|repo_root| TaskWorktree {
                repo_root,
                repo: worktree_repo,
                branch: worktree_branch,
                dir: worktree_dir,
            });
            Ok(TaskItem {
                id: r.get(0)?,
                text: r.get(1)?,
                status: r.get(2)?,
                position: r.get(3)?,
                created_at: r.get(4)?,
                completed_at: r.get(5)?,
                notes: r.get(6)?,
                outcome,
                archived_at,
                goal,
                worktree,
                issues: Vec::new(),
                prs: Vec::new(),
                closed: false,
                display_outcome: None,
                has_worktree: false,
            }
            .with_derived_fields())
        })?;
        let mut tasks = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        self.load_task_links(&mut tasks)?;
        Ok(tasks)
    }

    /// Fill `issues`/`prs` on already-mapped tasks. Loads both link tables
    /// whole (they are small — one row per attached ref) and distributes by
    /// `task_id`, keeping `(repo, number)` order deterministic.
    fn load_task_links(&self, tasks: &mut [TaskItem]) -> Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        use std::collections::HashMap;
        let mut issues: HashMap<i64, Vec<TaskIssueLink>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT task_id, repo, number, url, state FROM task_issues
                 ORDER BY task_id, repo, number",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    TaskIssueLink {
                        repo: r.get(1)?,
                        number: r.get(2)?,
                        url: r.get(3)?,
                        state: r.get(4)?,
                    },
                ))
            })?;
            for row in rows {
                let (task_id, link) = row?;
                issues.entry(task_id).or_default().push(link);
            }
        }
        let mut prs: HashMap<i64, Vec<TaskPrLink>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT task_id, repo, number, url, state, checks FROM task_prs
                 ORDER BY task_id, repo, number",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    TaskPrLink {
                        repo: r.get(1)?,
                        number: r.get(2)?,
                        url: r.get(3)?,
                        state: r.get(4)?,
                        checks: r.get(5)?,
                    },
                ))
            })?;
            for row in rows {
                let (task_id, link) = row?;
                prs.entry(task_id).or_default().push(link);
            }
        }
        for task in tasks.iter_mut() {
            if let Some(links) = issues.remove(&task.id) {
                task.issues = links;
            }
            if let Some(links) = prs.remove(&task.id) {
                task.prs = links;
            }
        }
        Ok(())
    }

    fn query_refs(&self, sql: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Error with [`Error::TaskNotFound`] unless a task with `id` exists.
    fn require_task(&self, id: i64) -> Result<()> {
        let exists = self.conn.prepare("SELECT 1 FROM tasks WHERE id = ?1")?.exists(params![id])?;
        if exists { Ok(()) } else { Err(Error::TaskNotFound(id)) }
    }
}
