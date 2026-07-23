//! SQLite-backed store for the towles-tool "personal dashboard" data: calendar
//! events, kanban todos, issues, PR status, and collector run bookkeeping.
//!
//! This crate is deliberately Tauri-free (the shared-crate rule): both the CLI and
//! the Tauri app depend on it. Clocks are injected as `now_ms` parameters (epoch
//! milliseconds) so logic stays deterministic under test.
//!
//! **Calendar events are the one exception to epoch-ms storage.** Their
//! `starts_at`/`ends_at` are RFC 3339 strings that keep the offset the calendar
//! reported (`2026-07-20T15:00:00+01:00`), because an epoch integer throws that
//! away — it can say *when* a meeting is but never that it was booked as 3pm
//! London. Everything else here (`updated_at`, run timestamps, task times) is
//! still epoch ms; see [`Store::replace_events_for_source`] for how the two meet.
//!
//! The public output structs serialize with `camelCase` keys to match the TypeScript
//! contract consumed by the frontend / Tauri commands.

use rusqlite::Connection;
use thiserror::Error;

pub mod attention;
pub use attention::{
    ChecksFailedEdge, ChecksFailedWatch, FAIL_STREAK, MeetingStartEdge, MeetingStartWatch,
    ReviewRequestedEdge, ReviewRequestedWatch, StaleCollectorEdge, StaleCollectorWatch,
    WatchedCollector,
};

mod collect;
mod events;
mod github;
mod model;
mod schema;
mod tasks;

pub use model::*;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("could not resolve a data directory")]
    NoDataDir,

    #[error("no task with id {0}")]
    TaskNotFound(i64),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A handle to the SQLite store.
pub struct Store {
    pub(crate) conn: Connection,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MCP_CALL_RETAIN, MCP_CALL_SNAPSHOT_LIMIT, utc_key};
    use crate::schema::SCHEMA_VERSION;
    use chrono::{DateTime, FixedOffset};
    use rusqlite::params;

    fn issue_link(repo: &str, number: i64, state: &str) -> TaskIssueLink {
        TaskIssueLink {
            repo: repo.to_string(),
            number,
            url: format!("https://github.com/{repo}/issues/{number}"),
            state: state.to_string(),
        }
    }

    #[test]
    fn gh_targets_entering_done_closes_open_issues() {
        let links = [issue_link("a/b", 1, "open"), issue_link("a/b", 2, "open")];
        let targets = gh_close_reopen_targets("doing", "done", &links);
        assert_eq!(targets, vec![("a/b".to_string(), 1, true), ("a/b".to_string(), 2, true)]);
    }

    #[test]
    fn gh_targets_leaving_done_reopens_closed_issues() {
        let links = [issue_link("a/b", 1, "closed")];
        let targets = gh_close_reopen_targets("done", "doing", &links);
        assert_eq!(targets, vec![("a/b".to_string(), 1, false)]);
    }

    #[test]
    fn gh_targets_skip_links_already_in_target_state() {
        // Entering done skips issues already closed; leaving done skips open ones.
        assert!(
            gh_close_reopen_targets("doing", "done", &[issue_link("a/b", 1, "closed")]).is_empty()
        );
        assert!(
            gh_close_reopen_targets("done", "doing", &[issue_link("a/b", 1, "open")]).is_empty()
        );
    }

    #[test]
    fn gh_targets_only_a_mix_when_entering_done() {
        // Only the still-open issue is closed; the already-closed one is left alone.
        let links = [issue_link("a/b", 1, "open"), issue_link("a/b", 2, "closed")];
        let targets = gh_close_reopen_targets("doing", "done", &links);
        assert_eq!(targets, vec![("a/b".to_string(), 1, true)]);
    }

    #[test]
    fn gh_targets_empty_when_status_unchanged() {
        assert!(
            gh_close_reopen_targets("done", "done", &[issue_link("a/b", 1, "open")]).is_empty()
        );
        assert!(
            gh_close_reopen_targets("doing", "doing", &[issue_link("a/b", 1, "open")]).is_empty()
        );
    }

    #[test]
    fn gh_targets_empty_for_moves_not_touching_done() {
        // Neither side is `done`, so no close/reopen regardless of link state.
        assert!(
            gh_close_reopen_targets("todo", "doing", &[issue_link("a/b", 1, "open")]).is_empty()
        );
        assert!(
            gh_close_reopen_targets("doing", "todo", &[issue_link("a/b", 1, "closed")]).is_empty()
        );
    }

    #[test]
    fn gh_targets_empty_for_link_less_tasks() {
        assert!(gh_close_reopen_targets("doing", "done", &[]).is_empty());
        assert!(gh_close_reopen_targets("done", "doing", &[]).is_empty());
    }

    /// Raw `task_issues` rows — the observable for delete-cascade tests, now
    /// that no production API exposes the table's contents directly.
    fn issue_link_rows(s: &Store) -> Vec<(i64, String, i64)> {
        let mut stmt = s
            .conn
            .prepare("SELECT task_id, repo, number FROM task_issues ORDER BY repo, number")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    /// Epoch ms -> the `DateTime<FixedOffset>` the event types now hold. UTC,
    /// since these tests assert on instants, not on presentation.
    fn at(ms: i64) -> DateTime<FixedOffset> {
        DateTime::from_timestamp_millis(ms).unwrap().fixed_offset()
    }

    fn event(ext: &str, start: i64) -> EventInput {
        EventInput {
            external_id: ext.to_string(),
            title: format!("Event {ext}"),
            start: at(start),
            end: Some(at(start + 1000)),
            attendees: vec!["a@example.com".to_string()],
            location: None,
            join_url: None,
        }
    }

    /// Write events for tests that don't care about source/day scoping: one
    /// source, a window wide enough to sweep everything. Tests that DO care
    /// about the scoping call [`Store::replace_events_for_source`] directly.
    fn put_events(s: &Store, events: &[EventInput], now_ms: i64) -> Result<usize> {
        s.replace_events_for_source("test", i64::MIN, i64::MAX, events, now_ms)
    }

    fn issue(repo: &str, number: i64, updated: i64) -> IssueInput {
        IssueInput {
            repo: repo.to_string(),
            number,
            title: format!("Issue {number}"),
            labels: vec!["bug".to_string()],
            state: "open".to_string(),
            url: format!("https://github.com/{repo}/issues/{number}"),
            updated_ts: updated,
        }
    }

    #[test]
    fn migrations_are_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let s = Store::open(&path).unwrap();
            s.add_task("survives", "backlog", None, 1).unwrap();
        }
        // Re-open: migrate runs again without error, data intact.
        let s = Store::open(&path).unwrap();
        assert_eq!(s.open_tasks().unwrap().len(), 1);
        let version: String = s
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION.to_string());
    }

    #[test]
    fn migrate_brings_pre_kanban_tasks_table_forward() {
        // Reproduces a db created before the day-screens pivot: `tasks` has the
        // old source/source_ref/done columns and no status/position/repo/
        // issue_number/issue_url, plus the since-removed `emails` table.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    source TEXT NOT NULL,
                    source_ref TEXT,
                    text TEXT NOT NULL,
                    due_ts INTEGER,
                    done INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER
                );
                CREATE TABLE emails (id INTEGER PRIMARY KEY);
                INSERT INTO tasks (source, text, done, created_at)
                    VALUES ('manual', 'old todo', 0, 1),
                           ('manual', 'finished todo', 1, 2);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let snapshot = s.snapshot().unwrap();
        assert_eq!(snapshot.tasks.len(), 2);
        assert!(snapshot.tasks.iter().any(|t| t.text == "old todo" && t.status == "backlog"));
        assert!(snapshot.tasks.iter().any(|t| t.text == "finished todo" && t.status == "done"));

        // Writes must work too: the legacy NOT-NULL `source` column has to be
        // gone, or every INSERT that omits it fails.
        let added = s.add_task("new todo", "backlog", None, 3).unwrap();
        assert_eq!(added.status, "backlog");
        assert!(!task_columns(&s).contains(&"source".to_string()));

        let has_emails: bool = s
            .conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'emails'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(!has_emails, "dead `emails` table should be dropped");
    }

    fn task_columns(s: &Store) -> Vec<String> {
        let mut stmt = s.conn.prepare("PRAGMA table_info(tasks)").unwrap();
        let cols = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
        cols.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    #[test]
    fn migrate_repairs_half_migrated_tasks_table() {
        // A db the old ALTER-based migration already touched: v2 columns exist,
        // but the legacy NOT-NULL `source` column is still present, so inserts
        // that omit it fail. The rebuild must keep the v2 values it finds.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    source TEXT NOT NULL,
                    source_ref TEXT,
                    text TEXT NOT NULL,
                    due_ts INTEGER,
                    done INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    repo TEXT,
                    issue_number INTEGER,
                    issue_url TEXT
                );
                INSERT INTO tasks (source, text, done, created_at, status, position, repo,
                                   issue_number, issue_url)
                    VALUES ('manual', 'linked todo', 0, 1, 'doing', 2, 'o/r', 7,
                            'https://github.com/o/r/issues/7');",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let t = s.snapshot().unwrap().tasks.into_iter().find(|t| t.text == "linked todo").unwrap();
        assert_eq!(t.status, "doing");
        assert_eq!(t.position, 2);
        // The old single link came through the v2 rebuild AND the v7 port
        // into the task_issues link table.
        assert_eq!(t.issues.len(), 1);
        assert_eq!(t.issues[0].repo, "o/r");
        assert_eq!(t.issues[0].number, 7);
        assert_eq!(t.issues[0].state, "open");
        s.add_task("post-repair todo", "backlog", None, 9).unwrap();
        assert!(!task_columns(&s).contains(&"source".to_string()));
        assert!(!task_columns(&s).contains(&"repo".to_string()));
    }

    /// `local_day_bounds` exists in one place because it scopes a DELETE, and
    /// two implementations had already drifted apart on DST handling — one
    /// widening to a ±1-day window, the other collapsing to an empty one, both
    /// feeding the same destructive call. These pin the properties that make
    /// the shared version safe to hand to a delete.
    #[test]
    fn local_day_bounds_is_a_single_day_that_contains_its_reference() {
        // A few instants spread across the year, including both DST changeover
        // weekends in the northern hemisphere.
        for reference in [
            1_700_000_000_000_i64, // Nov 2023
            1_678_600_000_000,     // Mar 2023 (spring forward)
            1_699_164_000_000,     // Nov 2023 (fall back)
            1_719_000_000_000,     // Jun 2024
            0,                     // epoch
        ] {
            let (start, end) = Store::local_day_bounds(reference);
            assert!(start <= reference, "window starts at or before its reference ({reference})");
            assert!(reference < end, "window contains its reference ({reference})");
            let span = end - start;
            // A civil day is 23, 24 or 25 hours long depending on DST. Never more.
            assert!(
                (23 * 3_600_000..=25 * 3_600_000).contains(&span),
                "span {span}ms for {reference} is not one civil day"
            );
        }
    }

    /// The fallback direction is the safety property: if the boundary can't be
    /// resolved, delete nothing rather than delete more. Stale rows are fixed by
    /// the next pull; over-deleted rows are gone.
    #[test]
    fn local_day_bounds_never_widens_past_a_day() {
        let (start, end) = Store::local_day_bounds(i64::MAX);
        assert!(end - start <= 25 * 3_600_000, "degenerate input must not widen the delete");
    }

    #[test]
    fn replace_events_swaps_within_one_source_and_day() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, 1000, &[event("a", 100), event("b", 200)], 1)
            .unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2);
        let n = s.replace_events_for_source("google", 0, 1000, &[event("c", 300)], 2).unwrap();
        assert_eq!(n, 1);
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 1, "the earlier pull for this source+day is swept");
        assert_eq!(events[0].external_id, "c");
        assert_eq!(events[0].source, "google", "source is recorded for provenance");
    }

    /// The reason this method exists: two calendars are pulled independently and
    /// merged into one timeline. Under the old full-table swap, whichever pulled
    /// second erased the first.
    #[test]
    fn one_sources_pull_never_disturbs_another() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, 1000, &[event("personal", 100)], 1).unwrap();
        s.replace_events_for_source("outlook", 0, 1000, &[event("work", 200)], 1).unwrap();

        let merged = s.snapshot().unwrap().events;
        let ids: Vec<&str> = merged.iter().map(|e| e.external_id.as_str()).collect();
        assert_eq!(ids, vec!["personal", "work"], "both calendars coexist, merged by start_ts");

        // Re-pulling one source replaces only its own lane.
        s.replace_events_for_source("google", 0, 1000, &[event("personal-v2", 100)], 2).unwrap();
        let events = s.snapshot().unwrap().events;
        let ids: Vec<&str> = events.iter().map(|e| e.external_id.as_str()).collect();
        assert_eq!(ids, vec!["personal-v2", "work"], "the work calendar survives untouched");
    }

    /// The delete is scoped to the day window too, so pulling today never drops
    /// a tomorrow's-events row that some other call stored.
    #[test]
    fn replace_events_leaves_other_days_alone() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, i64::MAX, &[event("tomorrow", 5_000)], 1).unwrap();
        s.replace_events_for_source("google", 0, 1000, &[event("today", 100)], 2).unwrap();

        let events = s.snapshot().unwrap().events;
        let ids: Vec<&str> = events.iter().map(|e| e.external_id.as_str()).collect();
        assert_eq!(ids, vec!["today", "tomorrow"], "out-of-window row untouched");
    }

    /// The scoped delete bounds one lane and one day, so something else has to
    /// bound the table over time — otherwise yesterday's meetings, and every row
    /// from a calendar the user renamed or removed, accumulate forever. The old
    /// full-table swap did that implicitly; this pins the replacement.
    #[test]
    fn old_events_are_swept_including_orphaned_lanes() {
        let s = Store::open_in_memory().unwrap();
        let now = 30 * EVENT_RETAIN_MS;
        let stale = now - EVENT_RETAIN_MS - 1;

        // A row from a lane that will never be written again (its source was
        // removed from settings), old enough to be past retention.
        s.replace_events_for_source(
            "retired-calendar",
            0,
            i64::MAX,
            &[event("ancient", stale)],
            now,
        )
        .unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 1);

        // A write to a *different* lane sweeps it — age, not source, is what
        // catches an orphan, since no per-source write will ever visit it again.
        s.replace_events_for_source("google", now, now + 86_400_000, &[event("today", now)], now)
            .unwrap();
        let ids: Vec<String> =
            s.snapshot().unwrap().events.iter().map(|e| e.external_id.clone()).collect();
        assert_eq!(ids, vec!["today"], "the orphaned lane's stale row is gone");
    }

    /// A repeated `externalId` inside one payload used to have the upsert
    /// overwrite its own earlier row mid-loop: one row landed, the other
    /// vanished, and the returned count still claimed both were written.
    #[test]
    fn duplicate_external_ids_in_one_payload_are_collapsed_and_counted_honestly() {
        let s = Store::open_in_memory().unwrap();
        let now = 30 * EVENT_RETAIN_MS;
        let mut first = event("abc", now + 1000);
        first.title = "Standup (9am)".to_string();
        let mut second = event("abc", now + 5000);
        second.title = "Standup (2pm)".to_string();

        let written = s
            .replace_events_for_source("google", now, now + 86_400_000, &[first, second], now)
            .unwrap();
        assert_eq!(written, 1, "count reflects rows that landed, not payload length");
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Standup (2pm)", "last occurrence wins, matching the upsert");
    }

    /// Retention can't be hostage to a write happening: switching the last
    /// calendar off means no write ever runs again, so the sweep has to be
    /// callable on its own or stale rows live forever.
    #[test]
    fn sweep_old_events_works_without_a_write() {
        let s = Store::open_in_memory().unwrap();
        let now = 30 * EVENT_RETAIN_MS;
        s.replace_events_for_source(
            "google",
            0,
            i64::MAX,
            &[
                event("stale", now - EVENT_RETAIN_MS - 1),
                event("fresh", now),
            ],
            now,
        )
        .unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2, "both written inside the window");

        // Time passes; nothing writes. The sweep alone must still age it out.
        let later = now + EVENT_RETAIN_MS;
        let removed = s.sweep_old_events(later).unwrap();
        assert_eq!(removed, 1);
        let ids: Vec<String> =
            s.snapshot().unwrap().events.iter().map(|e| e.external_id.clone()).collect();
        assert_eq!(ids, vec!["fresh"]);
    }

    /// Retention must not eat a meeting that hasn't happened yet, nor the recent
    /// past a countdown might still be reasoning about.
    #[test]
    fn retention_keeps_recent_and_future_events() {
        let s = Store::open_in_memory().unwrap();
        let now = 30 * EVENT_RETAIN_MS;
        s.replace_events_for_source(
            "google",
            0,
            i64::MAX,
            &[
                event("yesterday", now - 86_400_000),
                event("next-week", now + 7 * 86_400_000),
            ],
            now,
        )
        .unwrap();
        s.replace_events_for_source("outlook", now, now + 86_400_000, &[], now).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2, "both are inside the retention window");
    }

    /// Two providers can legitimately mint the same event id — that's why the
    /// uniqueness rule is `(source, external_id)` and not `external_id` alone.
    #[test]
    fn the_same_external_id_can_exist_in_two_sources() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, 1000, &[event("shared-id", 100)], 1).unwrap();
        s.replace_events_for_source("outlook", 0, 1000, &[event("shared-id", 200)], 1).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2, "no unique-constraint collision");
    }

    /// A row outside the swept window with a colliding id must upsert rather
    /// than blow up the whole pull on a constraint violation.
    #[test]
    fn re_pushing_an_out_of_window_id_updates_it_instead_of_failing() {
        let s = Store::open_in_memory().unwrap();
        s.replace_events_for_source("google", 0, i64::MAX, &[event("e", 9_000)], 1).unwrap();
        // Window doesn't cover 9_000, so the delete misses it; the insert collides.
        s.replace_events_for_source("google", 0, 1000, &[event("e", 9_000)], 2).unwrap();
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 1, "upserted, not duplicated");
    }

    #[test]
    fn replace_issues_is_full_swap_and_decodes_labels() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/r", 1, 100), issue("o/r", 2, 200)]).unwrap();
        assert_eq!(s.issues().unwrap().len(), 2);
        // Newest update first.
        assert_eq!(s.issues().unwrap()[0].number, 2);
        assert_eq!(s.issues().unwrap()[0].labels, vec!["bug".to_string()]);
        let n = s.replace_issues(&[issue("o/r", 3, 300)]).unwrap();
        assert_eq!(n, 1);
        let issues = s.issues().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 3);
    }

    #[test]
    fn replace_issues_for_repos_preserves_other_repos_rows() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/a", 1, 100), issue("o/b", 2, 200)]).unwrap();

        // Repo o/a re-collected (now empty); o/b's gh call failed → untouched.
        s.replace_issues_for_repos(&["o/a".to_string()], &[]).unwrap();
        let issues = s.issues().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].repo, "o/b");

        // Scoped write inserts the named repo's fresh rows.
        s.replace_issues_for_repos(&["o/a".to_string()], &[issue("o/a", 9, 900)]).unwrap();
        let issues = s.issues().unwrap();
        let repos: Vec<&str> = issues.iter().map(|i| i.repo.as_str()).collect();
        assert!(repos.contains(&"o/a") && repos.contains(&"o/b"));
    }

    #[test]
    fn reconcile_repos_upserts_and_drops_untracked() {
        let s = Store::open_in_memory().unwrap();
        s.reconcile_repos(
            &[
                ("/repo/a".to_string(), "o/a".to_string()),
                ("/repo/b".to_string(), "o/b".to_string()),
            ],
            100,
        )
        .unwrap();
        assert_eq!(s.repo_root_for_owner_repo("o/a").unwrap().as_deref(), Some("/repo/a"));
        assert_eq!(s.repo_root_for_owner_repo("o/b").unwrap().as_deref(), Some("/repo/b"));

        // /repo/a's origin was renamed and /repo/b fell out of tracking.
        s.reconcile_repos(&[("/repo/a".to_string(), "o/a-renamed".to_string())], 200).unwrap();
        assert_eq!(s.repo_root_for_owner_repo("o/a").unwrap(), None);
        assert_eq!(s.repo_root_for_owner_repo("o/a-renamed").unwrap().as_deref(), Some("/repo/a"));
        assert_eq!(s.repo_root_for_owner_repo("o/b").unwrap(), None);
    }

    #[test]
    fn reconcile_repos_empty_clears_the_cache() {
        let s = Store::open_in_memory().unwrap();
        s.reconcile_repos(&[("/repo/a".to_string(), "o/a".to_string())], 100).unwrap();
        s.reconcile_repos(&[], 200).unwrap();
        assert!(s.repo_slugs().unwrap().is_empty());
    }

    #[test]
    fn attach_detach_issue_links_and_get_issue() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/r", 1, 100)]).unwrap();
        let plain = s.add_task("plain task", "backlog", None, 1).unwrap();
        let linked = s.add_task("linked task", "backlog", None, 2).unwrap();
        s.attach_task_issue(linked.id, "o/r", 1, "https://github.com/o/r/issues/1").unwrap();
        s.attach_task_issue(linked.id, "o/r", 2, "https://github.com/o/r/issues/2").unwrap();

        let got = s.get_task(linked.id).unwrap().unwrap();
        assert_eq!(got.issues.len(), 2);
        assert_eq!(got.issues[0].state, "open");
        assert!(s.get_task(plain.id).unwrap().unwrap().issues.is_empty());

        // Re-attach refreshes the url but never resets collector-owned state.
        s.set_issue_link_state("o/r", 1, "closed", 5).unwrap();
        s.attach_task_issue(linked.id, "o/r", 1, "https://new.example/1").unwrap();
        let got = s.get_task(linked.id).unwrap().unwrap();
        let one = got.issues.iter().find(|l| l.number == 1).unwrap();
        assert_eq!(one.state, "closed");
        assert_eq!(one.url, "https://new.example/1");

        s.detach_task_issue(linked.id, "o/r", 2).unwrap();
        assert_eq!(s.get_task(linked.id).unwrap().unwrap().issues.len(), 1);
        // Detaching a non-existent link is a no-op, attaching to a missing task errors.
        s.detach_task_issue(linked.id, "o/r", 99).unwrap();
        assert!(matches!(s.attach_task_issue(9999, "o/r", 1, "u"), Err(Error::TaskNotFound(9999))));

        let found = s.get_issue("o/r", 1).unwrap().unwrap();
        assert_eq!(found.number, 1);
        assert!(s.get_issue("o/r", 999).unwrap().is_none());
    }

    #[test]
    fn worktree_binding_set_lookup_and_detach() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("worktree-backed", "doing", None, 1).unwrap();
        assert!(t.worktree.is_none());
        s.set_task_worktree(
            t.id,
            "/repos/x",
            Some("o/x"),
            Some("feat/y"),
            Some("/repos/x/.claude/worktrees/feat-y"),
        )
        .unwrap();

        let bound = s.task_for_worktree_dir("/repos/x/.claude/worktrees/feat-y").unwrap().unwrap();
        assert_eq!(bound.id, t.id);
        let worktree = bound.worktree.unwrap();
        assert_eq!(worktree.repo_root, "/repos/x");
        assert_eq!(worktree.repo.as_deref(), Some("o/x"));
        assert_eq!(worktree.branch.as_deref(), Some("feat/y"));

        // A repo-only rebind (the retry path re-sends the submit-time bind)
        // upserts: `None` means "leave as is", never "clear".
        s.set_task_worktree(t.id, "/repos/x", None, None, None).unwrap();
        let rebound = s.get_task(t.id).unwrap().unwrap().worktree.unwrap();
        assert_eq!(rebound.repo.as_deref(), Some("o/x"));
        assert_eq!(rebound.branch.as_deref(), Some("feat/y"));
        assert_eq!(rebound.dir.as_deref(), Some("/repos/x/.claude/worktrees/feat-y"));

        // Removing the worktree takes the whole task with it — there is no
        // detached-task state to land in (see `set_task_worktree`'s doc).
        s.delete_task(t.id).unwrap();
        assert!(s.task_for_worktree_dir("/repos/x/.claude/worktrees/feat-y").unwrap().is_none());
        assert!(s.get_task(t.id).unwrap().is_none());
        assert!(matches!(
            s.set_task_worktree(777, "/r", None, Some("b"), None),
            Err(Error::TaskNotFound(777))
        ));
    }

    #[test]
    fn refresh_link_states_from_cache_and_missing_refs() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("t", "doing", None, 1).unwrap();
        s.attach_task_issue(t.id, "o/r", 1, "u1").unwrap();
        s.attach_task_issue(t.id, "o/r", 2, "u2").unwrap();
        s.attach_task_pr(t.id, "o/r", 10, "p10").unwrap();

        // Issue 1 is in the snapshot (still open); issue 2 and PR 10 are not.
        s.replace_issues(&[issue("o/r", 1, 100)]).unwrap();
        s.refresh_link_states_from_cache(50).unwrap();

        assert_eq!(s.open_issue_refs_missing_from_cache().unwrap(), vec![("o/r".to_string(), 2)]);
        assert_eq!(s.open_pr_refs_missing_from_cache().unwrap(), vec![("o/r".to_string(), 10)]);

        // A targeted fetch resolves the misses; terminal states stop being
        // reported even though they remain absent from the snapshot.
        s.set_issue_link_state("o/r", 2, "closed", 60).unwrap();
        s.set_pr_link_state("o/r", 10, "merged", None, 60).unwrap();
        assert!(s.open_issue_refs_missing_from_cache().unwrap().is_empty());
        assert!(s.open_pr_refs_missing_from_cache().unwrap().is_empty());
        let got = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(got.issues.iter().find(|l| l.number == 2).unwrap().state, "closed");
        assert_eq!(got.prs[0].state, "merged");
    }

    #[test]
    fn auto_attach_worktree_prs_links_by_repo_and_branch() {
        let pr = |branch: &str, number: i64| PrInput {
            repo: "o/x".to_string(),
            number,
            title: "t".to_string(),
            branch: branch.to_string(),
            state: "open".to_string(),
            checks: "pending".to_string(),
            review_state: String::new(),
            url: format!("https://github.com/o/x/pull/{number}"),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("worktree task", "doing", None, 1).unwrap();
        s.set_task_worktree(t.id, "/repos/x", Some("o/x"), Some("feat/y"), Some("/w")).unwrap();
        let other = s.add_task("no worktree", "backlog", None, 2).unwrap();

        s.replace_prs(&[pr("feat/y", 7), pr("other-branch", 8)]).unwrap();
        let n = s.auto_attach_worktree_prs(9).unwrap();
        assert_eq!(n, 1);
        let got = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(got.prs.len(), 1);
        assert_eq!(got.prs[0].number, 7);
        assert!(s.get_task(other.id).unwrap().unwrap().prs.is_empty());

        // Idempotent: a second pass creates nothing new.
        assert_eq!(s.auto_attach_worktree_prs(10).unwrap(), 0);
    }

    #[test]
    fn replace_prs_for_repos_preserves_other_repos_rows() {
        let pr = |repo: &str, number: i64| PrInput {
            repo: repo.to_string(),
            number,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[pr("o/a", 1), pr("o/b", 2)]).unwrap();
        s.replace_prs_for_repos(&["o/a".to_string()], &[pr("o/a", 3)]).unwrap();
        let prs = s.prs().unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.repo == "o/b" && p.number == 2));
        assert!(prs.iter().any(|p| p.repo == "o/a" && p.number == 3));
    }

    #[test]
    fn replace_open_prs_for_repos_preserves_merged_rows() {
        let pr = |repo: &str, number: i64, state: &str| PrInput {
            repo: repo.to_string(),
            number,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: state.to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[pr("o/a", 1, "open"), pr("o/a", 2, "merged")]).unwrap();
        // A fresh open-only sweep must not delete the merged row it never fetched.
        s.replace_open_prs_for_repos(&["o/a".to_string()], &[pr("o/a", 3, "open")]).unwrap();
        let prs = s.prs().unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.number == 2 && p.state == "merged"));
        assert!(prs.iter().any(|p| p.number == 3 && p.state == "open"));
        assert!(!prs.iter().any(|p| p.number == 1));
    }

    #[test]
    fn replace_merged_prs_for_repos_preserves_open_rows() {
        let pr = |repo: &str, number: i64, state: &str| PrInput {
            repo: repo.to_string(),
            number,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: state.to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[pr("o/a", 1, "open"), pr("o/a", 2, "merged")]).unwrap();
        // A merged-only sweep must not delete the open row it never fetched.
        s.replace_merged_prs_for_repos(&["o/a".to_string()], &[pr("o/a", 4, "merged")]).unwrap();
        let prs = s.prs().unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.number == 1 && p.state == "open"));
        assert!(prs.iter().any(|p| p.number == 4 && p.state == "merged"));
        assert!(!prs.iter().any(|p| p.number == 2));
    }

    #[test]
    fn replace_open_prs_and_replace_merged_prs_are_full_snapshots_scoped_by_state() {
        let pr = |repo: &str, number: i64, state: &str| PrInput {
            repo: repo.to_string(),
            number,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: state.to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[pr("o/a", 1, "open"), pr("o/b", 2, "merged")]).unwrap();
        s.replace_open_prs(&[pr("o/a", 3, "open")]).unwrap();
        let prs = s.prs().unwrap();
        // The other repo's open row (none here) would be purged, but its merged
        // row survives; repo o/a's stale open row 1 is gone, replaced by 3.
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.number == 3 && p.state == "open"));
        assert!(prs.iter().any(|p| p.number == 2 && p.state == "merged"));

        s.replace_merged_prs(&[pr("o/b", 5, "merged")]).unwrap();
        let prs = s.prs().unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs.iter().any(|p| p.number == 3 && p.state == "open"));
        assert!(prs.iter().any(|p| p.number == 5 && p.state == "merged"));
    }

    #[test]
    fn replace_prs_round_trips_every_checks_state() {
        let pr = |number: i64, checks: &str| PrInput {
            repo: "o/r".to_string(),
            number,
            title: format!("pr {number}"),
            branch: format!("b{number}"),
            state: "open".to_string(),
            checks: checks.to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: number,
        };
        let s = Store::open_in_memory().unwrap();
        s.replace_prs(&[
            pr(1, "passing"),
            pr(2, "failing"),
            pr(3, "pending"),
            pr(4, "none"),
        ])
        .unwrap();
        let mut got: Vec<(i64, String)> =
            s.prs().unwrap().into_iter().map(|p| (p.number, p.checks)).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (1, "passing".to_string()),
                (2, "failing".to_string()),
                (3, "pending".to_string()),
                (4, "none".to_string()),
            ]
        );
    }

    #[test]
    fn add_task_lands_in_backlog_and_orders_by_position() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("first", "backlog", None, 100).unwrap();
        let b = s.add_task("second", "backlog", None, 200).unwrap();
        assert_eq!(a.status, "backlog");
        assert_eq!(a.position, 0);
        assert_eq!(b.position, 1);
        let open = s.open_tasks().unwrap();
        let texts: Vec<&str> = open.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, vec!["first", "second"]);
    }

    #[test]
    fn set_task_status_moves_columns_and_stamps_done() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("ship it", "backlog", None, 1).unwrap();
        s.set_task_status(t.id, "doing", 5).unwrap();
        let doing = s.open_tasks().unwrap();
        assert_eq!(doing[0].status, "doing");
        assert_eq!(doing[0].completed_at, None);

        s.set_task_status(t.id, "done", 20).unwrap();
        assert!(s.open_tasks().unwrap().is_empty());
        let done = s.snapshot().unwrap().tasks.into_iter().find(|x| x.id == t.id).unwrap();
        assert_eq!(done.status, "done");
        assert_eq!(done.completed_at, Some(20));

        // Re-opening clears completed_at.
        s.set_task_status(t.id, "backlog", 30).unwrap();
        let reopened = s.open_tasks().unwrap();
        assert_eq!(reopened[0].status, "backlog");
        assert_eq!(reopened[0].completed_at, None);
    }

    #[test]
    fn archive_closed_tasks_sweeps_only_old_finished() {
        let s = Store::open_in_memory().unwrap();
        let old = s.add_task("old done", "backlog", None, 1).unwrap();
        let abandoned = s.add_task("old abandoned", "doing", None, 1).unwrap();
        let recent = s.add_task("recent done", "backlog", None, 2).unwrap();
        let open = s.add_task("still open", "backlog", None, 3).unwrap();
        s.set_task_status(old.id, "done", 100).unwrap();
        s.close_task(abandoned.id, TaskOutcome::Abandoned, 200).unwrap();
        s.set_task_status(recent.id, "done", 5_000).unwrap();
        s.set_task_status(open.id, "doing", 4).unwrap();

        // Cutoff between the finished todos: the old done *and* the old
        // abandoned are archived — the rows survive, hidden, not deleted.
        let archived = s.archive_closed_tasks(1_000, 9_000).unwrap();
        assert_eq!(archived, 2);

        let tasks = s.snapshot().unwrap().tasks;
        let archived_ids: Vec<i64> =
            tasks.iter().filter(|t| t.archived_at.is_some()).map(|t| t.id).collect();
        assert!(archived_ids.contains(&old.id));
        assert!(archived_ids.contains(&abandoned.id));
        assert!(tasks.iter().any(|t| t.id == recent.id && t.archived_at.is_none()));
        assert!(tasks.iter().any(|t| t.id == open.id && t.archived_at.is_none()));

        // Nothing else old enough on a second sweep.
        assert_eq!(s.archive_closed_tasks(1_000, 9_001).unwrap(), 0);
    }

    #[test]
    fn close_task_as_done_lands_in_done_and_detaches_the_dir() {
        let s = Store::open_in_memory().unwrap();
        let done_first = s.add_task("already done", "done", None, 1).unwrap();
        let t = s.add_task("ship it", "doing", None, 2).unwrap();
        s.set_task_worktree(t.id, "/repos/x", Some("o/x"), Some("feat/y"), Some("/repos/x/wt"))
            .unwrap();

        let closed = s.close_task(t.id, TaskOutcome::Done, 500).unwrap();
        assert_eq!(closed.status, "done");
        assert_eq!(closed.outcome.as_deref(), Some("done"));
        assert_eq!(closed.completed_at, Some(500));
        assert!(closed.position > done_first.position, "appended to the done column");
        let wt = closed.worktree.expect("repo binding survives");
        assert_eq!(wt.branch.as_deref(), Some("feat/y"), "branch kept as historical fact");
        assert_eq!(wt.dir, None, "dir cleared — the worktree is gone");
        assert!(s.task_for_worktree_dir("/repos/x/wt").unwrap().is_none());
    }

    #[test]
    fn close_task_as_abandoned_freezes_status() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("didn't pan out", "doing", None, 1).unwrap();

        let closed = s.close_task(t.id, TaskOutcome::Abandoned, 500).unwrap();
        assert_eq!(closed.status, "doing", "status stays where the work stopped");
        assert_eq!(closed.outcome.as_deref(), Some("abandoned"));
        assert_eq!(closed.completed_at, Some(500), "stamped so the archive sweep can age it");
        assert!(!s.open_tasks().unwrap().iter().any(|x| x.id == t.id), "closed = not open");

        // Unknown outcomes never parse; a bad id is TaskNotFound.
        assert_eq!(TaskOutcome::parse("exploded"), None);
        assert_eq!(TaskOutcome::parse("abandoned"), Some(TaskOutcome::Abandoned));
        assert!(matches!(
            s.close_task(9999, TaskOutcome::Done, 501),
            Err(Error::TaskNotFound(9999))
        ));
    }

    #[test]
    fn status_move_out_of_done_reopens_a_closed_task() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("round two", "doing", None, 1).unwrap();
        s.close_task(t.id, TaskOutcome::Done, 100).unwrap();
        s.archive_task(t.id, 200).unwrap();

        // Dragging the card back to an active column clears the whole
        // terminal record: outcome, archive, completed_at.
        s.set_task_status(t.id, "doing", 300).unwrap();
        let back = s.task_by_id(t.id).unwrap();
        assert_eq!(back.outcome, None);
        assert_eq!(back.archived_at, None);
        assert_eq!(back.completed_at, None);

        // A move *within* done (re-close) keeps the record.
        s.close_task(t.id, TaskOutcome::Done, 400).unwrap();
        s.set_task_status(t.id, "done", 500).unwrap();
        assert_eq!(s.task_by_id(t.id).unwrap().outcome.as_deref(), Some("done"));
    }

    #[test]
    fn archive_and_unarchive_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("history", "doing", None, 1).unwrap();
        s.close_task(t.id, TaskOutcome::Abandoned, 100).unwrap();

        s.archive_task(t.id, 200).unwrap();
        assert_eq!(s.task_by_id(t.id).unwrap().archived_at, Some(200));
        // Idempotent: the original archive instant survives a re-archive.
        s.archive_task(t.id, 300).unwrap();
        assert_eq!(s.task_by_id(t.id).unwrap().archived_at, Some(200));

        s.unarchive_task(t.id).unwrap();
        let back = s.task_by_id(t.id).unwrap();
        assert_eq!(back.archived_at, None);
        assert_eq!(back.outcome.as_deref(), Some("abandoned"), "outcome survives unarchive");
        assert!(matches!(s.unarchive_task(9999), Err(Error::TaskNotFound(9999))));
    }

    #[test]
    fn auto_attach_skips_archived_tasks_but_not_closed_ones() {
        let s = Store::open_in_memory().unwrap();
        let closed = s.add_task("closed", "doing", None, 1).unwrap();
        s.set_task_worktree(closed.id, "/r/a", Some("o/r"), Some("feat/a"), None).unwrap();
        s.close_task(closed.id, TaskOutcome::Done, 10).unwrap();
        let archived = s.add_task("archived", "doing", None, 2).unwrap();
        s.set_task_worktree(archived.id, "/r/b", Some("o/r"), Some("feat/b"), None).unwrap();
        s.close_task(archived.id, TaskOutcome::Done, 10).unwrap();
        s.archive_task(archived.id, 20).unwrap();

        let pr = |number: i64, branch: &str| PrInput {
            repo: "o/r".to_string(),
            number,
            title: "t".to_string(),
            branch: branch.to_string(),
            state: "merged".to_string(),
            checks: "none".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: number,
        };
        s.replace_prs(&[pr(1, "feat/a"), pr(2, "feat/b")]).unwrap();

        assert_eq!(s.auto_attach_worktree_prs(30).unwrap(), 1);
        assert_eq!(s.task_by_id(closed.id).unwrap().prs.len(), 1, "closed still attaches");
        assert!(s.task_by_id(archived.id).unwrap().prs.is_empty(), "archived never attaches");
    }

    #[test]
    fn migrate_v13_adds_outcome_columns_to_a_v12_tasks_table() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            // A v12-era tasks table: full worktree columns, no outcome/archive.
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT,
                    worktree_repo_root TEXT,
                    worktree_repo TEXT,
                    worktree_branch TEXT,
                    worktree_dir TEXT
                );
                INSERT INTO tasks (text, status, position, created_at)
                    VALUES ('carried forward', 'doing', 0, 1);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].outcome, None, "pre-existing rows stay open");
        assert_eq!(tasks[0].archived_at, None);
        s.close_task(tasks[0].id, TaskOutcome::Abandoned, 10).unwrap();

        // Idempotent: reopening doesn't re-alter or lose the close.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.task_by_id(1).unwrap().outcome.as_deref(), Some("abandoned"));
    }

    #[test]
    fn migrate_v14_remaps_next_and_review_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT,
                    worktree_repo_root TEXT,
                    worktree_repo TEXT,
                    worktree_branch TEXT,
                    worktree_dir TEXT,
                    outcome TEXT,
                    archived_at INTEGER
                );
                INSERT INTO tasks (text, status, position, created_at)
                    VALUES ('was up next', 'next', 0, 1);
                INSERT INTO tasks (text, status, position, created_at)
                    VALUES ('was in review', 'review', 0, 2);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        let next = tasks.iter().find(|t| t.text == "was up next").unwrap();
        let review = tasks.iter().find(|t| t.text == "was in review").unwrap();
        assert_eq!(next.status, "backlog", "next folds back to not-started");
        assert_eq!(review.status, "doing", "review folds forward to in-progress");

        // Idempotent: reopening a db that already went through v14 is a no-op.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.task_by_id(next.id).unwrap().status, "backlog");
        assert_eq!(s.task_by_id(review.id).unwrap().status, "doing");
    }

    #[test]
    fn add_task_stores_notes_and_lands_in_requested_status() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("port the CLI", "backlog", Some("start with doctor"), 1).unwrap();
        assert_eq!(t.notes.as_deref(), Some("start with doctor"));
        assert!(t.issues.is_empty() && t.prs.is_empty() && t.worktree.is_none());
        // A worktree-backed task is born straight into `doing`.
        let d = s.add_task("agent already running", "doing", None, 2).unwrap();
        assert_eq!(d.status, "doing");
        assert_eq!(d.completed_at, None);
        // Unknown statuses are rejected.
        assert!(s.add_task("nope", "bogus", None, 3).is_err());
        let bare = s.add_task("no context", "backlog", None, 4).unwrap();
        assert_eq!(bare.notes, None);
    }

    #[test]
    fn migrate_adds_notes_column_to_pre_v4_tasks_table() {
        // A v2/v3-era db: kanban-shaped tasks table, but no `notes` column.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    due_ts INTEGER,
                    repo TEXT,
                    issue_number INTEGER,
                    issue_url TEXT,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER
                );
                INSERT INTO tasks (text, created_at) VALUES ('pre-v4 todo', 1);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        assert!(task_columns(&s).contains(&"notes".to_string()));
        let existing = s.open_tasks().unwrap();
        assert_eq!(existing[0].text, "pre-v4 todo");
        assert_eq!(existing[0].notes, None);
        let t = s.add_task("with notes", "backlog", Some("context"), 2).unwrap();
        assert_eq!(t.notes.as_deref(), Some("context"));
    }

    #[test]
    fn migrate_drops_retired_collector_rows_v6() {
        // A db carrying freshness rows from collectors removed in the
        // day-screens pivot alongside live ones.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let s = Store::open(&path).unwrap();
            for key in ["claude:email", "claude:tasks", "prs", "slack:dm"] {
                s.conn
                    .execute(
                        "INSERT INTO collect_runs (collector, ran_at, ok) VALUES (?1, 1, 1)",
                        params![key],
                    )
                    .unwrap();
            }
        }

        let s = Store::open(&path).unwrap();
        let keys: Vec<String> = s.runs().unwrap().into_iter().map(|r| r.collector).collect();
        assert_eq!(keys, ["prs", "slack:dm"], "retired collector keys are swept");
    }

    #[test]
    fn set_task_status_appends_to_end_of_target_column() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, 3).unwrap();

        // Moving into an empty column starts at 0; the next arrival lands after it.
        s.set_task_status(a.id, "doing", 10).unwrap();
        s.set_task_status(b.id, "doing", 11).unwrap();
        let pos = |id: i64, tasks: &[TaskItem]| tasks.iter().find(|t| t.id == id).unwrap().position;
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(a.id, &tasks), 0);
        assert_eq!(pos(b.id, &tasks), 1);

        // A later drop into the same column lands at the end, not at its old position.
        s.set_task_status(c.id, "doing", 12).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(c.id, &tasks), 2);

        // Bouncing a card out and back re-appends it after the survivors.
        s.set_task_status(a.id, "backlog", 13).unwrap();
        s.set_task_status(a.id, "doing", 14).unwrap();
        let tasks = s.snapshot().unwrap().tasks;
        assert_eq!(pos(a.id, &tasks), 3);
    }

    #[test]
    fn set_task_status_rejects_unknown() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("x", "backlog", None, 1).unwrap();
        assert!(s.set_task_status(t.id, "bogus", 2).is_err());
    }

    /// The ids in `status`'s column, in board (displayed) order.
    #[cfg(test)]
    fn column_ids(s: &Store, status: &str) -> Vec<i64> {
        s.snapshot()
            .unwrap()
            .tasks
            .into_iter()
            .filter(|t| t.status == status)
            .map(|t| t.id)
            .collect()
    }

    #[test]
    fn set_task_position_reorders_within_a_column() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, 3).unwrap();
        // Column starts [a, b, c] at positions 0,1,2.
        assert_eq!(column_ids(&s, "backlog"), vec![a.id, b.id, c.id]);

        // Move c to the top.
        s.set_task_position(c.id, "backlog", 0, 10).unwrap();
        assert_eq!(column_ids(&s, "backlog"), vec![c.id, a.id, b.id]);

        // Move c to the bottom (index past the end clamps to last).
        s.set_task_position(c.id, "backlog", 99, 11).unwrap();
        assert_eq!(column_ids(&s, "backlog"), vec![a.id, b.id, c.id]);

        // Move a into the middle.
        s.set_task_position(a.id, "backlog", 1, 12).unwrap();
        assert_eq!(column_ids(&s, "backlog"), vec![b.id, a.id, c.id]);

        // Positions are contiguous 0..n after each move.
        let positions: Vec<i64> = {
            let mut ts = s.snapshot().unwrap().tasks;
            ts.sort_by_key(|t| t.position);
            ts.into_iter().map(|t| t.position).collect()
        };
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[test]
    fn set_task_position_moves_across_columns_preserving_order() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        s.set_task_status(a.id, "doing", 3).unwrap();
        s.set_task_status(b.id, "doing", 4).unwrap();
        // doing = [a, b].
        let c = s.add_task("c", "backlog", None, 5).unwrap();

        // Drop c between a and b.
        s.set_task_position(c.id, "doing", 1, 6).unwrap();
        assert_eq!(column_ids(&s, "doing"), vec![a.id, c.id, b.id]);
        assert!(column_ids(&s, "backlog").is_empty());
    }

    #[test]
    fn set_task_position_stamps_and_clears_done() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("ship", "backlog", None, 1).unwrap();
        s.set_task_position(t.id, "done", 0, 20).unwrap();
        let done = s.snapshot().unwrap().tasks.into_iter().find(|x| x.id == t.id).unwrap();
        assert_eq!(done.status, "done");
        assert_eq!(done.completed_at, Some(20));

        s.set_task_position(t.id, "backlog", 0, 30).unwrap();
        let reopened = s.open_tasks().unwrap();
        assert_eq!(reopened[0].status, "backlog");
        assert_eq!(reopened[0].completed_at, None);
    }

    #[test]
    fn set_task_position_is_stable_under_repeated_moves() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        let c = s.add_task("c", "backlog", None, 3).unwrap();
        // Dropping a card onto its own position leaves the order unchanged.
        for _ in 0..5 {
            s.set_task_position(b.id, "backlog", 1, 10).unwrap();
        }
        assert_eq!(column_ids(&s, "backlog"), vec![a.id, b.id, c.id]);
    }

    #[test]
    fn set_task_position_rejects_unknown_status_and_missing_id() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("x", "backlog", None, 1).unwrap();
        assert!(s.set_task_position(t.id, "bogus", 0, 2).is_err());
        assert!(matches!(
            s.set_task_position(9999, "backlog", 0, 2),
            Err(Error::TaskNotFound(9999))
        ));
    }

    #[test]
    fn attach_task_issue_stores_reference() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("wire up board", "backlog", None, 1).unwrap();
        s.attach_task_issue(t.id, "o/r", 42, "https://github.com/o/r/issues/42").unwrap();
        let linked = s.open_tasks().unwrap()[0].clone();
        assert_eq!(linked.issues.len(), 1);
        assert_eq!(linked.issues[0].repo, "o/r");
        assert_eq!(linked.issues[0].number, 42);
        assert_eq!(linked.issues[0].url, "https://github.com/o/r/issues/42");
    }

    #[test]
    fn update_task_edits_text_and_notes() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("rough draft", "backlog", None, 1).unwrap();
        let updated = s.update_task(t.id, "polished", Some("ship friday")).unwrap();
        assert_eq!(updated.text, "polished");
        assert_eq!(updated.notes.as_deref(), Some("ship friday"));
        // Status/position are untouched by an edit.
        assert_eq!(updated.status, "backlog");
        assert_eq!(updated.position, t.position);
        // And it persists.
        assert_eq!(s.get_task(t.id).unwrap().unwrap().text, "polished");
    }

    #[test]
    fn update_task_none_notes_clears_them() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("call dentist", "backlog", Some("weds am"), 1).unwrap();
        assert_eq!(t.notes.as_deref(), Some("weds am"));
        // Passing None clears notes back out — a full replace, no sentinel.
        let cleared = s.update_task(t.id, "call dentist", None).unwrap();
        assert_eq!(cleared.notes, None);
    }

    #[test]
    fn update_task_nonexistent_errors() {
        let s = Store::open_in_memory().unwrap();
        let err = s.update_task(999, "ghost", None).unwrap_err();
        assert!(matches!(err, Error::TaskNotFound(999)));
    }

    #[test]
    fn delete_task_removes_row() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("keep", "backlog", None, 1).unwrap();
        let b = s.add_task("toss", "backlog", None, 2).unwrap();
        s.delete_task(b.id).unwrap();
        let open = s.open_tasks().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, a.id);
        assert!(s.get_task(b.id).unwrap().is_none());
    }

    #[test]
    fn delete_task_nonexistent_errors() {
        let s = Store::open_in_memory().unwrap();
        let err = s.delete_task(999).unwrap_err();
        assert!(matches!(err, Error::TaskNotFound(999)));
    }

    #[test]
    fn events_between_windows_by_start() {
        let s = Store::open_in_memory().unwrap();
        put_events(&s, &[event("a", 100), event("b", 300), event("c", 500)], 1).unwrap();
        let win = s.events_between(150, 500).unwrap();
        assert_eq!(win.iter().map(|e| e.external_id.as_str()).collect::<Vec<_>>(), vec!["b"]);
    }

    #[test]
    fn current_or_next_event_across_the_meeting_lifecycle() {
        // The `event` helper spans [start, start + 1000). Two non-overlapping
        // meetings: "b" runs [300, 1300), "c" runs [1500, 2500).
        let s = Store::open_in_memory().unwrap();
        put_events(&s, &[event("b", 300), event("c", 1500)], 1).unwrap();

        // Future: before it starts, "b" is the next meeting.
        assert_eq!(s.current_or_next_event(200).unwrap().unwrap().external_id, "b");
        // At the exact start it is already live.
        assert_eq!(s.current_or_next_event(300).unwrap().unwrap().external_id, "b");
        // In progress (start <= now < end): "b" stays selected, not skipped.
        assert_eq!(s.current_or_next_event(800).unwrap().unwrap().external_id, "b");
        // Ended (now >= end_ts): "b" drops out and the next meeting "c" takes over.
        assert_eq!(s.current_or_next_event(1300).unwrap().unwrap().external_id, "c");
        // After the last meeting ends there is nothing left.
        assert!(s.current_or_next_event(3000).unwrap().is_none());
    }

    #[test]
    fn current_or_next_event_without_end_is_a_point_in_time() {
        let s = Store::open_in_memory().unwrap();
        put_events(
            &s,
            &[EventInput {
                external_id: "no-end".to_string(),
                title: "Open-ended".to_string(),
                start: at(500),
                end: None,
                attendees: vec![],
                location: None,
                join_url: None,
            }],
            1,
        )
        .unwrap();
        // With no duration there is no live window: shown up to its start, then gone.
        assert_eq!(s.current_or_next_event(400).unwrap().unwrap().external_id, "no-end");
        assert_eq!(s.current_or_next_event(500).unwrap().unwrap().external_id, "no-end");
        assert!(s.current_or_next_event(600).unwrap().is_none());
    }

    #[test]
    fn record_run_upserts_per_collector() {
        let s = Store::open_in_memory().unwrap();
        s.record_run("gcal", true, None, 10).unwrap();
        s.record_run("gcal", false, Some("boom"), 20).unwrap();
        let runs = s.runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].ran_at, 20);
        assert!(!runs[0].ok);
        assert_eq!(runs[0].message.as_deref(), Some("boom"));
    }

    #[test]
    fn upsert_dm_preserves_dismissal_until_a_newer_message() {
        let s = Store::open_in_memory().unwrap();
        let msg = |ts: i64, from_me: bool| DmInput {
            channel: "D123".to_string(),
            from_name: "Sarah".to_string(),
            text: format!("msg at {ts}"),
            ts,
            from_me,
            url: Some("slack://channel?team=T1&id=D123".to_string()),
        };

        s.upsert_dm(&msg(100, false), 1).unwrap();
        let dm = &s.dms().unwrap()[0];
        assert!(!dm.from_me);
        assert_eq!(dm.dismissed_ts, 0, "fresh message starts undismissed");

        // Mark handled: dismissed_ts catches up to ts.
        s.dismiss_dm("D123", 100).unwrap();
        assert_eq!(s.dms().unwrap()[0].dismissed_ts, 100);

        // Re-collecting the same message keeps the dismissal.
        s.upsert_dm(&msg(100, false), 2).unwrap();
        let dm = s.dms().unwrap()[0].clone();
        assert_eq!(dm.dismissed_ts, 100);
        assert_eq!(dm.fetched_at, 2);

        // A newer message outruns the dismissal (dismissed_ts < ts again).
        s.upsert_dm(&msg(200, false), 3).unwrap();
        let dm = s.dms().unwrap()[0].clone();
        assert_eq!(dm.ts, 200);
        assert!(dm.dismissed_ts < dm.ts);

        // Replying clears it collector-side: latest message is mine.
        s.upsert_dm(&msg(300, true), 4).unwrap();
        assert!(s.dms().unwrap()[0].from_me);
    }

    #[test]
    fn dismiss_item_survives_replace_until_the_item_updates() {
        let s = Store::open_in_memory().unwrap();
        let pr = |updated_ts: i64| PrInput {
            repo: "octo/widgets".to_string(),
            number: 42,
            title: "feat: treemap".to_string(),
            branch: "feat/treemap".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: "review_requested".to_string(),
            url: "https://github.com/octo/widgets/pull/42".to_string(),
            updated_ts,
        };

        s.replace_prs(&[pr(100)]).unwrap();
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 0, "fresh PR starts undismissed");

        s.dismiss_item("pr", "octo/widgets", 42, 100).unwrap();
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 100);

        // A collector re-run with no real change (same updated_ts) keeps the
        // dismissal, exactly like a re-sent DM at the same ts.
        s.replace_prs(&[pr(100)]).unwrap();
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 100);

        // The PR actually changing (a newer review) outruns the dismissal.
        s.replace_prs(&[pr(200)]).unwrap();
        let pr_row = &s.prs().unwrap()[0];
        assert_eq!(pr_row.updated_ts, 200);
        assert!(pr_row.dismissed_ts < pr_row.updated_ts);
    }

    #[test]
    fn clear_dismissals_removes_every_kind() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[IssueInput {
            repo: "octo/widgets".to_string(),
            number: 118,
            title: "Flaky resize".to_string(),
            labels: vec![],
            state: "open".to_string(),
            url: "https://github.com/octo/widgets/issues/118".to_string(),
            updated_ts: 50,
        }])
        .unwrap();
        s.replace_prs(&[PrInput {
            repo: "octo/widgets".to_string(),
            number: 42,
            title: "feat: treemap".to_string(),
            branch: "feat/treemap".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: "review_requested".to_string(),
            url: "https://github.com/octo/widgets/pull/42".to_string(),
            updated_ts: 100,
        }])
        .unwrap();

        s.dismiss_item("issue", "octo/widgets", 118, 50).unwrap();
        s.dismiss_item("pr", "octo/widgets", 42, 100).unwrap();
        assert_eq!(s.issues().unwrap()[0].dismissed_ts, 50);
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 100);

        let cleared = s.clear_dismissals().unwrap();
        assert_eq!(cleared, 2);
        assert_eq!(s.issues().unwrap()[0].dismissed_ts, 0);
        assert_eq!(s.prs().unwrap()[0].dismissed_ts, 0);
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let s = Store::open_in_memory().unwrap();
        put_events(
            &s,
            &[EventInput {
                external_id: "x".to_string(),
                title: "T".to_string(),
                start: at(1),
                end: Some(at(2)),
                attendees: vec!["a@b.com".to_string()],
                location: Some("room".to_string()),
                join_url: Some("https://meet".to_string()),
            }],
            1,
        )
        .unwrap();
        s.add_task("do thing", "backlog", None, 1).unwrap();
        s.replace_issues(&[issue("o/r", 5, 6)]).unwrap();
        s.replace_prs(&[PrInput {
            repo: "o/r".to_string(),
            number: 7,
            title: "Fix".to_string(),
            branch: "feat".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: "approved".to_string(),
            url: "https://x".to_string(),
            updated_ts: 3,
        }])
        .unwrap();
        s.record_run("gcal", true, None, 4).unwrap();
        s.upsert_dm(
            &DmInput {
                channel: "D1".to_string(),
                from_name: "Sarah".to_string(),
                text: "hi".to_string(),
                ts: 5,
                from_me: false,
                url: None,
            },
            6,
        )
        .unwrap();

        let json = serde_json::to_string(&s.snapshot().unwrap()).unwrap();
        for key in [
            "\"start\"",
            "\"externalId\"",
            "\"joinUrl\"",
            "\"createdAt\"",
            "\"updatedTs\"",
            "\"reviewState\"",
            "\"ranAt\"",
            "\"fromName\"",
            "\"fromMe\"",
            "\"dismissedTs\"",
        ] {
            assert!(json.contains(key), "expected {key} in snapshot JSON: {json}");
        }
        // snake_case must not leak through.
        assert!(!json.contains("start_ts"));
        assert!(!json.contains("review_state"));
        // Event times are RFC 3339 on the wire, not epoch integers — this is
        // the readability the format exists for, so pin the rendered shape.
        // Note chrono's serde renders a zero offset as `Z` where `to_rfc3339`
        // writes `+00:00`. Both are valid RFC 3339 and parse identically, and
        // the generated sort column normalizes either — so this pins the shape
        // without pretending the two spellings must match.
        assert!(
            json.contains("\"start\":\"1970-01-01T00:00:00.001Z\""),
            "event start should be RFC 3339: {json}"
        );
    }

    /// `utc_key` and the `starts_at_utc` generated column must produce byte-
    /// identical strings. They are compared lexically, so a divergence in
    /// width or precision does not error — it silently returns the wrong rows
    /// from every range query. Pinned through SQLite itself rather than by
    /// eyeballing two format strings that live in different languages.
    #[test]
    fn utc_key_matches_the_generated_column() {
        let s = Store::open_in_memory().unwrap();
        for ms in [0i64, 1, 999, 1_700_000_000_000, -86_400_000] {
            s.replace_events_for_source("k", i64::MIN, i64::MAX, &[event("x", ms)], ms + 1)
                .unwrap();
            let stored: String = s
                .conn
                .query_row("SELECT starts_at_utc FROM events WHERE source = 'k'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(stored, utc_key(ms), "format drift at {ms}");
        }
    }

    /// The offset the calendar reported survives a write/read round trip —
    /// the whole reason these columns are text and not integers.
    #[test]
    fn a_non_utc_offset_round_trips_and_still_sorts_by_instant() {
        let s = Store::open_in_memory().unwrap();
        let london = DateTime::parse_from_rfc3339("2026-07-20T15:00:00+01:00").unwrap();
        let chicago = DateTime::parse_from_rfc3339("2026-07-20T09:30:00-05:00").unwrap();
        // Chicago 09:30-05:00 is 14:30Z — half an hour *before* London 15:00+01:00
        // (14:00Z)... no: 14:30Z is after 14:00Z. Instant order is london, chicago.
        s.replace_events_for_source(
            "tz",
            i64::MIN,
            i64::MAX,
            &[
                EventInput {
                    external_id: "chicago".to_string(),
                    title: "Standup".to_string(),
                    start: chicago,
                    end: None,
                    attendees: vec![],
                    location: None,
                    join_url: None,
                },
                EventInput {
                    external_id: "london".to_string(),
                    title: "Review".to_string(),
                    start: london,
                    end: None,
                    attendees: vec![],
                    location: None,
                    join_url: None,
                },
            ],
            london.timestamp_millis(),
        )
        .unwrap();

        let all = s.events_between(i64::MIN, i64::MAX).unwrap();
        // Sorted by instant (14:00Z then 14:30Z), NOT by the authored strings —
        // lexically "09:30-05:00" would come first and be wrong.
        assert_eq!(
            all.iter().map(|e| e.external_id.as_str()).collect::<Vec<_>>(),
            vec!["london", "chicago"]
        );
        // ...and each keeps the offset it was written with.
        let stored_london = all.iter().find(|e| e.external_id == "london").unwrap();
        assert_eq!(stored_london.start.to_rfc3339(), "2026-07-20T15:00:00+01:00");
        assert_eq!(stored_london.start.offset().local_minus_utc(), 3600);
    }

    fn mcp_call(method: &str, tool: Option<&str>, ok: bool) -> McpCallInput {
        McpCallInput {
            method: method.to_string(),
            tool: tool.map(str::to_string),
            args: tool.map(|_| "{\"title\":\"x\"}".to_string()),
            ok,
            error: (!ok).then(|| "boom".to_string()),
            duration_ms: Some(3),
            client: Some("claude-code 2.0".to_string()),
        }
    }

    #[test]
    fn record_mcp_call_reads_back_newest_first() {
        let s = Store::open_in_memory().unwrap();
        s.record_mcp_call(&mcp_call("tools/list", None, true), 10).unwrap();
        s.record_mcp_call(&mcp_call("tools/call", Some("todo_create"), true), 20).unwrap();
        s.record_mcp_call(&mcp_call("tools/call", Some("nope"), false), 30).unwrap();

        let calls = s.mcp_calls(10).unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].ts, 30);
        assert_eq!(calls[0].tool.as_deref(), Some("nope"));
        assert!(!calls[0].ok);
        assert_eq!(calls[0].error.as_deref(), Some("boom"));
        assert_eq!(calls[2].method, "tools/list");
        assert_eq!(calls[2].tool, None);
        assert_eq!(calls[1].args.as_deref(), Some("{\"title\":\"x\"}"));
        assert_eq!(calls[1].client.as_deref(), Some("claude-code 2.0"));

        // The limit caps the read.
        assert_eq!(s.mcp_calls(2).unwrap().len(), 2);
    }

    #[test]
    fn record_mcp_call_prunes_beyond_retention() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..(MCP_CALL_RETAIN + 25) {
            s.record_mcp_call(&mcp_call("ping", None, true), i).unwrap();
        }
        let calls = s.mcp_calls(MCP_CALL_RETAIN as usize * 2).unwrap();
        assert_eq!(calls.len(), MCP_CALL_RETAIN as usize);
        // The survivors are the newest rows.
        assert_eq!(calls[0].ts, MCP_CALL_RETAIN + 24);
        assert_eq!(calls.last().unwrap().ts, 25);
    }

    #[test]
    fn snapshot_carries_mcp_calls_camel_cased() {
        let s = Store::open_in_memory().unwrap();
        s.record_mcp_call(&mcp_call("tools/call", Some("day_brief"), true), 7).unwrap();
        let snapshot = s.snapshot().unwrap();
        assert_eq!(snapshot.mcp_calls.len(), 1);
        assert_eq!(snapshot.mcp_calls[0].tool.as_deref(), Some("day_brief"));

        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"mcpCalls\""), "expected mcpCalls in {json}");
        assert!(json.contains("\"durationMs\""), "expected durationMs in {json}");
        assert!(!json.contains("mcp_calls"));
    }

    #[test]
    fn archive_closed_tasks_never_sweeps_legacy_null_completed_at() {
        // A `done` row whose `completed_at` is NULL (data from before the column
        // was stamped) has no known completion time, so the sweep must skip it —
        // even with a cutoff far in the future. There is no public API to make
        // such a row, so insert it directly.
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute(
                "INSERT INTO tasks (text, status, position, created_at, completed_at)
                 VALUES ('legacy done', 'done', 0, 1, NULL)",
                [],
            )
            .unwrap();
        let normal = s.add_task("normal done", "backlog", None, 2).unwrap();
        s.set_task_status(normal.id, "done", 10).unwrap();

        let archived = s.archive_closed_tasks(1_000_000, 1_000_001).unwrap();
        assert_eq!(archived, 1, "only the stamped done row is swept");
        let visible: Vec<String> = s
            .snapshot()
            .unwrap()
            .tasks
            .into_iter()
            .filter(|t| t.archived_at.is_none())
            .map(|t| t.text)
            .collect();
        assert_eq!(visible, vec!["legacy done".to_string()]);
    }

    #[test]
    fn events_between_is_start_inclusive_end_exclusive() {
        let s = Store::open_in_memory().unwrap();
        put_events(
            &s,
            &[
                event("at-start", 100),
                event("mid", 150),
                event("at-end", 200),
            ],
            1,
        )
        .unwrap();
        // Window [100, 200): the event exactly at start is in, the one at end is out.
        let ids: Vec<String> =
            s.events_between(100, 200).unwrap().into_iter().map(|e| e.external_id).collect();
        assert_eq!(ids, vec!["at-start".to_string(), "mid".to_string()]);
    }

    #[test]
    fn current_or_next_event_on_empty_store_is_none() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.current_or_next_event(0).unwrap().is_none());
    }

    #[test]
    fn replace_events_round_trips_attendees_json() {
        let s = Store::open_in_memory().unwrap();
        put_events(
            &s,
            &[
                EventInput {
                    external_id: "many".to_string(),
                    title: "Sync".to_string(),
                    start: at(100),
                    end: Some(at(200)),
                    attendees: vec!["a@x.com".to_string(), "b@x.com".to_string()],
                    location: Some("Room 1".to_string()),
                    join_url: Some("https://meet/x".to_string()),
                },
                EventInput {
                    external_id: "none".to_string(),
                    title: "Solo".to_string(),
                    start: at(300),
                    end: None,
                    attendees: vec![],
                    location: None,
                    join_url: None,
                },
            ],
            1,
        )
        .unwrap();
        let events = s.snapshot().unwrap().events;
        let many = events.iter().find(|e| e.external_id == "many").unwrap();
        assert_eq!(many.attendees, vec!["a@x.com".to_string(), "b@x.com".to_string()]);
        assert_eq!(many.location.as_deref(), Some("Room 1"));
        let none = events.iter().find(|e| e.external_id == "none").unwrap();
        assert!(none.attendees.is_empty());
        assert_eq!(none.end, None);
    }

    #[test]
    fn open_tasks_orders_across_columns_by_board_order() {
        let s = Store::open_in_memory().unwrap();
        s.add_task("backlog item", "backlog", None, 1).unwrap();
        let doing = s.add_task("doing item", "backlog", None, 2).unwrap();
        let done = s.add_task("done item", "backlog", None, 3).unwrap();
        s.set_task_status(doing.id, "doing", 11).unwrap();
        s.set_task_status(done.id, "done", 13).unwrap();

        // open_tasks excludes done and returns backlog → doing.
        let statuses: Vec<String> = s.open_tasks().unwrap().into_iter().map(|t| t.status).collect();
        assert_eq!(statuses, vec!["backlog".to_string(), "doing".to_string()]);
    }

    #[test]
    fn snapshot_tasks_place_done_column_last() {
        let s = Store::open_in_memory().unwrap();
        let d = s.add_task("finish", "backlog", None, 1).unwrap();
        s.add_task("start", "backlog", None, 2).unwrap();
        s.set_task_status(d.id, "done", 10).unwrap();
        // Snapshot keeps done rows but orders them after open columns regardless
        // of insertion/completion order.
        let statuses: Vec<String> =
            s.snapshot().unwrap().tasks.into_iter().map(|t| t.status).collect();
        assert_eq!(statuses, vec!["backlog".to_string(), "done".to_string()]);
    }

    #[test]
    fn snapshot_caps_mcp_calls_at_the_snapshot_limit() {
        let s = Store::open_in_memory().unwrap();
        // More rows than the snapshot carries, but within retention.
        let total = MCP_CALL_SNAPSHOT_LIMIT + 20;
        for i in 0..total {
            s.record_mcp_call(&mcp_call("ping", None, true), i as i64).unwrap();
        }
        let snapshot = s.snapshot().unwrap();
        assert_eq!(snapshot.mcp_calls.len(), MCP_CALL_SNAPSHOT_LIMIT);
        // Newest first: the last recorded call heads the list.
        assert_eq!(snapshot.mcp_calls[0].ts, (total - 1) as i64);
    }

    #[test]
    fn replace_issues_and_prs_with_empty_clears_all_rows() {
        let s = Store::open_in_memory().unwrap();
        s.replace_issues(&[issue("o/r", 1, 100)]).unwrap();
        s.replace_prs(&[PrInput {
            repo: "o/r".to_string(),
            number: 2,
            title: "t".to_string(),
            branch: "b".to_string(),
            state: "open".to_string(),
            checks: "passing".to_string(),
            review_state: String::new(),
            url: "https://x".to_string(),
            updated_ts: 1,
        }])
        .unwrap();
        assert_eq!(s.replace_issues(&[]).unwrap(), 0);
        assert_eq!(s.replace_prs(&[]).unwrap(), 0);
        assert!(s.issues().unwrap().is_empty());
        assert!(s.prs().unwrap().is_empty());
    }

    /// v10 converts epoch-ms event rows to RFC 3339 text **without losing
    /// them**, unlike v9's deliberate drop. The instant is known exactly here;
    /// only the authored offset isn't, and `Z` says that honestly. Dropping
    /// instead would blank the next-meeting countdown until something writes —
    /// and with the pull collector off by default, that may be a long time.
    #[test]
    fn migrate_v10_converts_epoch_rows_to_rfc3339_keeping_them() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        // A v9-shaped db: has `source`, still epoch integers.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE events (
                    id INTEGER PRIMARY KEY,
                    source TEXT NOT NULL,
                    external_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    start_ts INTEGER NOT NULL,
                    end_ts INTEGER,
                    attendees TEXT NOT NULL DEFAULT '[]',
                    location TEXT,
                    join_url TEXT,
                    updated_at INTEGER NOT NULL,
                    UNIQUE(source, external_id)
                );
                INSERT INTO events
                    (source, external_id, title, start_ts, end_ts, updated_at)
                    VALUES
                    ('google', 'kept', 'Standup', 1700000000000, 1700001800000, 1),
                    ('google', 'no-end', 'Reminder', 1700003600000, NULL, 1);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let events = s.snapshot().unwrap().events;
        assert_eq!(events.len(), 2, "rows are converted, not dropped");

        let kept = events.iter().find(|e| e.external_id == "kept").unwrap();
        assert_eq!(kept.start_ms(), 1_700_000_000_000, "the instant survives exactly");
        assert_eq!(kept.end_ms(), Some(1_700_001_800_000));
        // Unknown authored zone becomes UTC, stated as such rather than guessed.
        assert_eq!(kept.start.offset().local_minus_utc(), 0);
        assert_eq!(kept.start.to_rfc3339(), "2023-11-14T22:13:20+00:00");

        let no_end = events.iter().find(|e| e.external_id == "no-end").unwrap();
        assert_eq!(no_end.end, None, "a NULL end stays NULL, not epoch 0");

        // The rebuilt table still writes, sorts and enforces its unique key.
        s.replace_events_for_source("outlook", i64::MIN, i64::MAX, &[event("kept", 1)], 2).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 3, "same id in another lane is fine");

        // Idempotent: reopening must not rebuild again and lose the rows.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 3, "no-op on a v10 db");
    }

    /// v9 rebuilds `events` for the `source` column and the composite unique
    /// key. Pre-v9 rows are **intentionally dropped** — the old schema recorded
    /// no source, and a row tagged with a guessed source would never be swept by
    /// any real pull, lingering in the countdown forever. Pinned as a test so
    /// the data loss stays a decision rather than a surprise.
    #[test]
    fn migrate_v9_rebuilds_events_and_drops_sourceless_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE events (
                    id INTEGER PRIMARY KEY,
                    external_id TEXT NOT NULL UNIQUE,
                    title TEXT NOT NULL,
                    start_ts INTEGER NOT NULL,
                    end_ts INTEGER,
                    attendees TEXT NOT NULL DEFAULT '[]',
                    location TEXT,
                    join_url TEXT,
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO events (external_id, title, start_ts, updated_at)
                    VALUES ('legacy', 'Old meeting', 100, 1);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let cols: Vec<String> = {
            let mut stmt = s.conn.prepare("PRAGMA table_info(events)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert!(cols.contains(&"source".to_string()), "source column added");
        assert!(s.snapshot().unwrap().events.is_empty(), "sourceless rows dropped, not guessed");

        // The rebuilt table takes writes and enforces the new composite key.
        s.replace_events_for_source("google", 0, 1000, &[event("a", 100)], 2).unwrap();
        s.replace_events_for_source("outlook", 0, 1000, &[event("a", 200)], 2).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2);

        // Idempotent: reopening doesn't rebuild again and lose the new rows.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.snapshot().unwrap().events.len(), 2, "migration is a no-op on a v9 db");
    }

    #[test]
    fn update_task_leaves_links_and_worktree_intact() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("wire board", "backlog", None, 1).unwrap();
        s.attach_task_issue(t.id, "o/r", 7, "https://github.com/o/r/issues/7").unwrap();
        s.set_task_worktree(t.id, "/repos/r", Some("o/r"), Some("feat/wire"), Some("/w")).unwrap();
        let updated = s.update_task(t.id, "wire board v2", Some("note")).unwrap();
        assert_eq!(updated.text, "wire board v2");
        // Editing free-form fields must not disturb links or the worktree binding.
        assert_eq!(updated.issues.len(), 1);
        assert_eq!(updated.issues[0].number, 7);
        assert_eq!(updated.worktree.unwrap().branch.as_deref(), Some("feat/wire"));
    }

    #[test]
    fn attach_task_issue_accumulates_links() {
        let s = Store::open_in_memory().unwrap();
        let t = s.add_task("multi-issue", "backlog", None, 1).unwrap();
        s.attach_task_issue(t.id, "o/a", 1, "https://github.com/o/a/issues/1").unwrap();
        s.attach_task_issue(t.id, "o/b", 2, "https://github.com/o/b/issues/2").unwrap();
        let got = s.get_task(t.id).unwrap().unwrap();
        // Attaching a second issue adds a link — it no longer overwrites.
        assert_eq!(got.issues.len(), 2);
        let repos: Vec<&str> = got.issues.iter().map(|l| l.repo.as_str()).collect();
        assert_eq!(repos, vec!["o/a", "o/b"]);
    }

    #[test]
    fn delete_task_cascades_link_rows_but_archiving_keeps_them() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_task("a", "backlog", None, 1).unwrap();
        let b = s.add_task("b", "backlog", None, 2).unwrap();
        s.attach_task_issue(a.id, "o/r", 1, "u").unwrap();
        s.attach_task_pr(a.id, "o/r", 2, "u").unwrap();
        s.attach_task_issue(b.id, "o/r", 3, "u").unwrap();

        s.delete_task(a.id).unwrap();
        assert_eq!(issue_link_rows(&s), vec![(b.id, "o/r".to_string(), 3)]);
        assert!(s.linked_pr_refs().unwrap().is_empty());

        // The archive sweep keeps the row, so its links survive too.
        s.set_task_status(b.id, "done", 10).unwrap();
        assert_eq!(s.archive_closed_tasks(100, 101).unwrap(), 1);
        assert_eq!(issue_link_rows(&s), vec![(b.id, "o/r".to_string(), 3)]);
    }

    #[test]
    fn migrate_v7_ports_single_link_and_drops_link_columns() {
        // A v5-era db: kanban tasks table with the single-issue link columns
        // and one linked + one bare todo.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    due_ts INTEGER,
                    repo TEXT,
                    issue_number INTEGER,
                    issue_url TEXT,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT
                );
                INSERT INTO tasks (text, status, position, repo, issue_number, issue_url,
                                   created_at, notes)
                    VALUES ('linked', 'doing', 1, 'o/r', 7,
                            'https://github.com/o/r/issues/7', 1, 'ctx'),
                           ('bare', 'backlog', 0, NULL, NULL, NULL, 2, NULL);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let cols = task_columns(&s);
        for gone in ["repo", "issue_number", "issue_url"] {
            assert!(!cols.contains(&gone.to_string()), "column {gone} should be dropped");
        }
        for added in [
            "worktree_repo_root",
            "worktree_repo",
            "worktree_branch",
            "worktree_dir",
        ] {
            assert!(cols.contains(&added.to_string()), "column {added} should exist");
        }

        let tasks = s.all_tasks().unwrap();
        let linked = tasks.iter().find(|t| t.text == "linked").unwrap();
        assert_eq!(linked.status, "doing");
        assert_eq!(linked.notes.as_deref(), Some("ctx"));
        assert_eq!(linked.issues.len(), 1);
        assert_eq!(linked.issues[0].repo, "o/r");
        assert_eq!(linked.issues[0].number, 7);
        assert_eq!(linked.issues[0].url, "https://github.com/o/r/issues/7");
        assert_eq!(linked.issues[0].state, "open");
        let bare = tasks.iter().find(|t| t.text == "bare").unwrap();
        assert!(bare.issues.is_empty());

        // Idempotent: re-open runs migrate again without duplicating links.
        drop(s);
        let s = Store::open(&path).unwrap();
        let linked = s.all_tasks().unwrap().into_iter().find(|t| t.text == "linked").unwrap();
        assert_eq!(linked.issues.len(), 1);
    }

    #[test]
    fn migrate_v8_drops_due_column_keeping_rows() {
        // A v7-era db: current shape plus the retired due_ts column.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    due_ts INTEGER,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT,
                    worktree_repo_root TEXT,
                    worktree_repo TEXT,
                    worktree_branch TEXT,
                    worktree_dir TEXT
                );
                INSERT INTO tasks (text, status, position, due_ts, created_at, notes)
                    VALUES ('was due', 'doing', 1, 1752200000000, 1, 'ctx'),
                           ('never due', 'backlog', 0, NULL, 2, NULL);",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        assert!(!task_columns(&s).contains(&"due_ts".to_string()), "due_ts should be dropped");
        let tasks = s.all_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        let kept = tasks.iter().find(|t| t.text == "was due").unwrap();
        assert_eq!(kept.status, "doing");
        assert_eq!(kept.notes.as_deref(), Some("ctx"));

        // Idempotent: a second open finds no due_ts column and is a no-op.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.all_tasks().unwrap().len(), 2);
    }

    #[test]
    fn migrate_v11_renames_slot_columns_to_worktree_keeping_bindings() {
        // A v7-era db with the pre-rename slot_* columns and a bound task.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tt.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'backlog',
                    position INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    notes TEXT,
                    slot_repo_root TEXT,
                    slot_repo TEXT,
                    slot_branch TEXT,
                    slot_dir TEXT
                );
                INSERT INTO tasks (text, status, position, created_at,
                                   slot_repo_root, slot_repo, slot_branch, slot_dir)
                    VALUES ('bound', 'doing', 0, 1,
                            '/repos/x', 'o/x', 'feat/y', '/repos/x/wt');",
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let cols = task_columns(&s);
        assert!(cols.contains(&"worktree_repo_root".to_string()), "columns renamed");
        assert!(!cols.iter().any(|c| c.starts_with("slot_")), "no slot_* columns remain");
        let task = s.all_tasks().unwrap().into_iter().find(|t| t.text == "bound").unwrap();
        let wt = task.worktree.expect("binding survives the rename");
        assert_eq!(wt.repo_root, "/repos/x");
        assert_eq!(wt.branch.as_deref(), Some("feat/y"));
        assert_eq!(wt.dir.as_deref(), Some("/repos/x/wt"));

        // Idempotent: a second open finds no slot_* columns and is a no-op.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.all_tasks().unwrap().len(), 1);
    }
}
