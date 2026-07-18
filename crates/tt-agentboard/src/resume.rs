//! Crash recovery: after the app dies without a clean exit, work out which
//! Claude sessions were running in which panes so the user can pick which to
//! relaunch with `claude --resume`.
//!
//! A candidate needs two independent facts to line up:
//!
//! 1. **tt had a pane running it** — [`crate::sessions::SessionRecord`]'s
//!    `last_claude_session_id`, persisted while the agent was live because live
//!    attribution reads `TT_SESSION_ID` from `/proc` and dies with the process.
//! 2. **The transcript is still on disk** — it supplies the authoritative
//!    recency (file mtime) and the title, and `claude --resume` needs it anyway.
//!
//! A Claude session run outside tt fails (1) and is never offered; a pane whose
//! transcript was deleted fails (2) and could not be resumed regardless.
//!
//! Clock, paths and pid-liveness are all parameters so the decisions are
//! unit-testable without a real crash.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sessions::SessionRecord;

/// How far back from the crash a transcript may have been touched and still be
/// offered. Wide enough to survive an overnight reboot, narrow enough that a
/// week-old session is never suggested unprompted.
pub const DEFAULT_RESUME_WINDOW_MS: i64 = 12 * 60 * 60 * 1000;

/// How often the running app refreshes its heartbeat.
pub const HEARTBEAT_INTERVAL_MS: i64 = 30_000;

/// Slack above the estimated crash time before a transcript counts as "touched
/// after the crash".
///
/// The estimate *is* the last heartbeat, so it lags the real crash by up to one
/// interval — and the session you were mid-thought in is the one still being
/// written in that gap. Comparing against the bare estimate would reject
/// exactly the candidate this exists to offer.
pub const CRASH_TIME_SLACK_MS: i64 = HEARTBEAT_INTERVAL_MS * 2;

/// Written at startup, refreshed by a heartbeat, flipped to `clean_exit` on an
/// orderly shutdown. A stale `clean_exit: false` is what "we crashed" means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMarker {
    pub pid: u32,
    pub started_at_ms: i64,
    /// Last time the app proved it was alive; doubles as the estimated crash
    /// time, since a crash leaves no other timestamp behind.
    pub heartbeat_ms: i64,
    pub clean_exit: bool,
}

/// How the previous run ended. A missing marker counts as [`PriorRun::Clean`] —
/// there is nothing to offer either way, so it needs no variant of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriorRun {
    Clean,
    Crashed { at_ms: i64 },
}

/// `<agentboard_dir>/runtime.json` — instance-scoped like the rest of the
/// agentboard run state (a slot's crash is not the daily driver's crash).
pub fn default_runtime_path() -> PathBuf {
    tt_config::agentboard_dir_lossy().join("runtime.json")
}

/// Classify the previous run.
///
/// A dirty marker whose pid is *still alive* is not a crash — it's a second
/// instance running concurrently, and treating it as one would offer to resume
/// sessions that instance is actively using.
pub fn classify_prior(prior: Option<&RunMarker>, pid_alive: bool) -> PriorRun {
    match prior {
        Some(m) if !m.clean_exit && !pid_alive => PriorRun::Crashed { at_ms: m.heartbeat_ms },
        _ => PriorRun::Clean,
    }
}

/// Read the marker (`None` on missing/corrupt — both mean "nothing to trust").
pub fn read_marker(path: &Path) -> Option<RunMarker> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The single writer for the marker file. Callers own `started_at_ms` (the app
/// keeps it from `begin_run`), so refreshing a heartbeat costs one write rather
/// than a read to recover a value the caller already has.
pub fn write_marker(path: &Path, marker: &RunMarker) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(marker).unwrap_or_else(|_| "{}".to_string());
    crate::persist::write_atomic(path, &format!("{json}\n"))
}

/// Classify whatever the last run left behind, then claim the marker for this
/// one. `pid_alive` is injected so the caller supplies a cross-platform probe.
pub fn begin_run(path: &Path, pid: u32, now_ms: i64, pid_alive: impl Fn(u32) -> bool) -> PriorRun {
    let prior = read_marker(path);
    let verdict = classify_prior(prior.as_ref(), prior.as_ref().is_some_and(|m| pid_alive(m.pid)));
    let _ = write_marker(
        path,
        &RunMarker { pid, started_at_ms: now_ms, heartbeat_ms: now_ms, clean_exit: false },
    );
    verdict
}

/// A transcript located on disk. Carries the mtime so the cheap recency filter
/// can run before anything parses the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRef {
    pub path: PathBuf,
    pub mtime_ms: i64,
}

/// One resumable pane: the picker row, plus the two ids the relaunch needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeCandidate {
    pub folder_dir: String,
    /// tt's PTY session id (`SessionRecord.id`) — the pane to restore into.
    pub pane_id: String,
    pub pane_name: String,
    /// The thread id to hand to `claude --resume`.
    pub claude_session_id: String,
    pub title: Option<String>,
    /// Transcript mtime — when this session was last worked on.
    pub last_active_ms: i64,
}

/// Build the candidate list from persisted pane records, newest first.
///
/// Deliberately two-phase: `locate` only stats the file, and `title` — which
/// parses it — runs *only* for records that survive the recency filter.
/// Transcripts reach tens of megabytes, so parsing before filtering would read
/// and deserialize a whole thread just to discard it.
pub fn select_candidates<'a, L, T>(
    records: impl Iterator<Item = (&'a str, &'a SessionRecord)>,
    crashed_at_ms: i64,
    window_ms: i64,
    locate: L,
    title: T,
) -> Vec<ResumeCandidate>
where
    L: Fn(&str, &str) -> Option<TranscriptRef>,
    T: Fn(&TranscriptRef) -> Option<String>,
{
    let cutoff = crashed_at_ms - window_ms;
    let mut out: Vec<ResumeCandidate> = records
        .filter_map(|(dir, rec)| {
            let claude_session_id = rec.last_claude_session_id.as_deref()?;
            let found = locate(dir, claude_session_id)?;
            // Too old to be what you were doing, or — well after the crash —
            // owned by some other live process that resuming here would fight.
            if found.mtime_ms < cutoff || found.mtime_ms > crashed_at_ms + CRASH_TIME_SLACK_MS {
                return None;
            }
            Some(ResumeCandidate {
                folder_dir: dir.to_string(),
                pane_id: rec.id.clone(),
                pane_name: rec.name.clone(),
                claude_session_id: claude_session_id.to_string(),
                title: title(&found),
                last_active_ms: found.mtime_ms,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_active_ms.cmp(&a.last_active_ms).then_with(|| a.pane_id.cmp(&b.pane_id))
    });
    out
}

/// Find a session's transcript and stat it. No parsing — see
/// [`select_candidates`].
pub fn locate_transcript(
    projects_dir: &Path,
    cwd: &str,
    claude_session_id: &str,
) -> Option<TranscriptRef> {
    let path = crate::watchers::claude_code::find_journal(projects_dir, cwd, claude_session_id)?;
    let mtime_ms = std::fs::metadata(&path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some(TranscriptRef { path, mtime_ms })
}

/// The transcript's own title, if it names itself. Parses the file, so it is
/// called only for candidates that already passed the recency filter.
pub fn transcript_title(found: &TranscriptRef) -> Option<String> {
    tt_claude_code::session_title_file(&found.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    const DEAD: fn(u32) -> bool = |_| false;

    fn rec(id: &str, name: &str, claude: Option<&str>) -> SessionRecord {
        SessionRecord {
            id: id.to_string(),
            name: name.to_string(),
            created_at: 0,
            purpose: None,
            last_claude_session_id: claude.map(str::to_string),
        }
    }

    /// Stands in for the transcripts on disk: only pairs present here resolve,
    /// so an absent entry models "no transcript".
    type Disk = HashMap<(String, String), (Option<String>, i64)>;

    fn disk(entries: &[(&str, &str, Option<&str>, i64)]) -> Disk {
        entries
            .iter()
            .map(|(dir, sid, title, mtime)| {
                ((dir.to_string(), sid.to_string()), (title.map(str::to_string), *mtime))
            })
            .collect()
    }

    /// `locate` keys the fake path by session id so `title` can find it back.
    fn locate_from(map: &Disk) -> impl Fn(&str, &str) -> Option<TranscriptRef> + '_ {
        move |dir, sid| {
            map.get(&(dir.to_string(), sid.to_string())).map(|(_, mtime)| TranscriptRef {
                path: PathBuf::from(format!("/fake/{sid}.jsonl")),
                mtime_ms: *mtime,
            })
        }
    }

    fn title_from(map: &Disk) -> impl Fn(&TranscriptRef) -> Option<String> + '_ {
        move |found| {
            let sid = found.path.file_stem()?.to_str()?;
            map.iter().find(|((_, s), _)| s == sid).and_then(|(_, (title, _))| title.clone())
        }
    }

    fn select(
        records: &[(String, SessionRecord)],
        crashed_at: i64,
        d: &Disk,
    ) -> Vec<ResumeCandidate> {
        select_candidates(
            records.iter().map(|(dir, r)| (dir.as_str(), r)),
            crashed_at,
            DEFAULT_RESUME_WINDOW_MS,
            locate_from(d),
            title_from(d),
        )
    }

    #[test]
    fn a_clean_or_missing_marker_offers_nothing() {
        assert_eq!(classify_prior(None, false), PriorRun::Clean);
        let clean = RunMarker { pid: 1, started_at_ms: 0, heartbeat_ms: 50, clean_exit: true };
        assert_eq!(classify_prior(Some(&clean), false), PriorRun::Clean);
    }

    #[test]
    fn dirty_marker_with_dead_pid_is_a_crash_at_the_last_heartbeat() {
        let dirty = RunMarker { pid: 1, started_at_ms: 0, heartbeat_ms: 900, clean_exit: false };
        assert_eq!(classify_prior(Some(&dirty), false), PriorRun::Crashed { at_ms: 900 });
    }

    #[test]
    fn dirty_marker_with_live_pid_is_a_concurrent_instance_not_a_crash() {
        // Otherwise a second window offers to resume the sessions the first is
        // actively using.
        let dirty = RunMarker { pid: 1, started_at_ms: 0, heartbeat_ms: 900, clean_exit: false };
        assert_eq!(classify_prior(Some(&dirty), true), PriorRun::Clean);
    }

    #[test]
    fn begin_run_reports_the_crash_then_claims_the_marker() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("runtime.json");
        write_marker(
            &path,
            &RunMarker { pid: 999_999, started_at_ms: 0, heartbeat_ms: 900, clean_exit: false },
        )
        .unwrap();

        assert_eq!(begin_run(&path, 4242, 1000, DEAD), PriorRun::Crashed { at_ms: 900 });

        let now = read_marker(&path).unwrap();
        assert_eq!(now.pid, 4242);
        assert!(!now.clean_exit);

        // A launch after a clean exit stays silent.
        write_marker(
            &path,
            &RunMarker { pid: 4242, started_at_ms: 1000, heartbeat_ms: 1100, clean_exit: true },
        )
        .unwrap();
        assert_eq!(begin_run(&path, 5555, 1200, DEAD), PriorRun::Clean);
    }

    #[test]
    fn a_pane_with_a_live_transcript_is_offered_with_its_title() {
        let records = vec![("/r/a".to_string(), rec("pane1", "shell 1", Some("c1")))];
        let d = disk(&[("/r/a", "c1", Some("fix the parser"), 9_000)]);

        let got = select(&records, 10_000, &d);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].pane_id, "pane1");
        assert_eq!(got[0].claude_session_id, "c1");
        assert_eq!(got[0].title.as_deref(), Some("fix the parser"));
        assert_eq!(got[0].last_active_ms, 9_000);
    }

    #[test]
    fn a_pane_that_never_ran_claude_is_not_offered() {
        let records = vec![("/r/a".to_string(), rec("pane1", "shell 1", None))];
        assert!(select(&records, 10_000, &disk(&[])).is_empty());
    }

    #[test]
    fn a_pane_whose_transcript_is_gone_is_not_offered() {
        // `claude --resume` on a deleted transcript fails, so offering it would
        // be a broken button.
        let records = vec![("/r/a".to_string(), rec("pane1", "shell 1", Some("c1")))];
        assert!(select(&records, 10_000, &disk(&[])).is_empty());
    }

    #[test]
    fn transcripts_outside_the_window_are_not_offered() {
        let records = vec![
            ("/r/a".to_string(), rec("stale", "shell 1", Some("c-old"))),
            ("/r/a".to_string(), rec("future", "shell 2", Some("c-new"))),
        ];
        let d = disk(&[
            ("/r/a", "c-old", None, 10_000 - DEFAULT_RESUME_WINDOW_MS - 1),
            ("/r/a", "c-new", None, 10_000 + CRASH_TIME_SLACK_MS + 1),
        ]);
        assert!(select(&records, 10_000, &d).is_empty());
    }

    #[test]
    fn a_transcript_written_just_after_the_last_heartbeat_is_still_offered() {
        // The crash estimate is the last heartbeat, so the session being
        // written when the app died has an mtime after it. Rejecting that as
        // "touched after the crash" would gut the feature.
        let records = vec![("/r/a".to_string(), rec("pane1", "shell 1", Some("c1")))];
        let d = disk(&[("/r/a", "c1", None, 10_000 + HEARTBEAT_INTERVAL_MS)]);
        assert_eq!(select(&records, 10_000, &d).len(), 1);
    }

    #[test]
    fn the_expensive_title_read_is_skipped_for_filtered_out_records() {
        // The whole point of the two-phase split: a stale record must not cost
        // a transcript parse.
        use std::cell::Cell;
        let records = vec![("/r/a".to_string(), rec("stale", "shell 1", Some("c-old")))];
        let d = disk(&[("/r/a", "c-old", Some("ancient"), 0)]);
        let titles_read = Cell::new(0);
        let got = select_candidates(
            records.iter().map(|(dir, r)| (dir.as_str(), r)),
            10_000 + DEFAULT_RESUME_WINDOW_MS * 2,
            DEFAULT_RESUME_WINDOW_MS,
            locate_from(&d),
            |f| {
                titles_read.set(titles_read.get() + 1);
                title_from(&d)(f)
            },
        );
        assert!(got.is_empty());
        assert_eq!(titles_read.get(), 0, "filtered-out record must not be parsed");
    }

    #[test]
    fn candidates_come_back_newest_first() {
        let records = vec![
            ("/r/a".to_string(), rec("old", "shell 1", Some("c1"))),
            ("/r/b".to_string(), rec("new", "shell 2", Some("c2"))),
            ("/r/c".to_string(), rec("mid", "shell 3", Some("c3"))),
        ];
        let d = disk(&[
            ("/r/a", "c1", None, 1_000),
            ("/r/b", "c2", None, 9_000),
            ("/r/c", "c3", None, 5_000),
        ]);
        let got = select(&records, 10_000, &d);
        let ids: Vec<&str> = got.iter().map(|c| c.pane_id.as_str()).collect();
        assert_eq!(ids, ["new", "mid", "old"]);
    }

    #[test]
    fn locate_and_title_read_a_real_file() {
        let dir = TempDir::new().unwrap();
        let projects = dir.path().join("projects");
        let encoded = projects.join("-r-a");
        std::fs::create_dir_all(&encoded).unwrap();
        std::fs::write(
            encoded.join("c1.jsonl"),
            "{\"type\":\"user\",\"sessionId\":\"c1\",\"customTitle\":\"ship it\"}\n",
        )
        .unwrap();

        let found = locate_transcript(&projects, "/r/a", "c1").unwrap();
        assert!(found.mtime_ms > 0);
        assert_eq!(transcript_title(&found).as_deref(), Some("ship it"));

        assert!(locate_transcript(&projects, "/r/a", "missing").is_none());
    }
}
