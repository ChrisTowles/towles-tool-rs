//! Session discovery on disk and bar-chart aggregation. Ports
//! `src/commands/graph/sessions.ts`.
//!
//! All filesystem functions take an explicit `projects_dir` so tests never read
//! the real `~/.claude`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Local};

use tt_claude_code::{session_title_file, usage_totals_file};

use crate::analyzer::{analyze_session, extract_project_name};
use crate::parser::calculate_cutoff_ms;
use crate::types::{BarChartData, BarChartDay, ModelBar, ProjectBar, SessionResult};
use crate::{Error, Result};
use tt_claude_code::parse_transcript_file;

/// Modification time of a file, in ms since the Unix epoch.
fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Local `YYYY-MM-DD` date for a mtime in ms.
fn local_date(mtime_ms: i64) -> String {
    let dt: DateTime<Local> =
        DateTime::from(UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms as u64));
    dt.format("%Y-%m-%d").to_string()
}

/// Find recent sessions from the projects directory, most-recent first, limited
/// to `limit` and (optionally) the last `days`. Ports `findRecentSessions`.
pub fn find_recent_sessions(
    projects_dir: &Path,
    limit: usize,
    days: f64,
    now_ms: i64,
) -> Result<Vec<SessionResult>> {
    let mut sessions: Vec<SessionResult> = Vec::new();
    let cutoff_ms = calculate_cutoff_ms(days, now_ms);

    for project_entry in std::fs::read_dir(projects_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project = project_entry.file_name().to_string_lossy().to_string();

        for file_entry in std::fs::read_dir(&project_path)? {
            let file_entry = file_entry?;
            let file_path = file_entry.path();
            let name = file_entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".jsonl") {
                continue;
            }
            let meta = file_entry.metadata()?;
            let mtime = mtime_ms(&meta);

            if cutoff_ms > 0 && mtime < cutoff_ms {
                continue;
            }

            let session_id = name.trim_end_matches(".jsonl").to_string();
            let tokens = usage_totals_file(&file_path).billable();
            let title = session_title_file(&file_path);

            sessions.push(SessionResult {
                session_id,
                path: file_path,
                date: local_date(mtime),
                tokens,
                project: project.clone(),
                mtime,
                title,
            });
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.mtime));
    sessions.truncate(limit);
    Ok(sessions)
}

/// Find the file path for a specific session ID. Ports `findSessionPath`.
pub fn find_session_path(projects_dir: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    for project_entry in std::fs::read_dir(projects_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let jsonl_path = project_path.join(format!("{session_id}.jsonl"));
        if jsonl_path.exists() {
            return Ok(Some(jsonl_path));
        }
    }
    Ok(None)
}

/// The `SessionResult` metadata needed to build the file path/date for a single
/// session ID (used by the JSON/CSV single-session path in the CLI).
pub fn session_result_for_path(session_id: &str, path: &Path) -> Result<SessionResult> {
    let meta = std::fs::metadata(path).map_err(Error::Io)?;
    let mtime = mtime_ms(&meta);
    let project = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(SessionResult {
        session_id: session_id.to_string(),
        path: path.to_path_buf(),
        date: local_date(mtime),
        tokens: 0,
        project,
        mtime,
        title: session_title_file(path),
    })
}

/// Build bar-chart data from session results, grouped by date then project.
/// Ports `buildBarChartData`.
pub fn build_bar_chart_data(sessions: &[SessionResult]) -> BarChartData {
    if sessions.is_empty() {
        return BarChartData { days: Vec::new() };
    }

    // date -> ordered list of (project, tokens), preserving first-seen order.
    let mut by_date: BTreeMap<String, Vec<(String, i64)>> = BTreeMap::new();

    for session in sessions {
        let project = extract_project_name(&session.project);
        let projects = by_date.entry(session.date.clone()).or_default();
        if let Some(existing) = projects.iter_mut().find(|(p, _)| *p == project) {
            existing.1 += session.tokens;
        } else {
            projects.push((project, session.tokens));
        }
    }

    let days: Vec<BarChartDay> = by_date
        .into_iter()
        .map(|(date, mut projects)| {
            projects.sort_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
            BarChartDay {
                date,
                projects: projects
                    .into_iter()
                    .map(|(project, total_tokens)| ProjectBar { project, total_tokens })
                    .collect(),
            }
        })
        .collect();

    BarChartData { days }
}

/// Total tokens per project across all sessions, sorted descending. Cheap: the
/// per-session `tokens` total already comes from the lightweight discovery
/// pass in [`find_recent_sessions`], no re-parse needed.
pub fn build_project_totals(sessions: &[SessionResult]) -> Vec<ProjectBar> {
    let mut order: Vec<String> = Vec::new();
    let mut totals: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    for session in sessions {
        let project = extract_project_name(&session.project);
        totals.entry(project.clone()).and_modify(|t| *t += session.tokens).or_insert_with(|| {
            order.push(project);
            session.tokens
        });
    }

    let mut bars: Vec<ProjectBar> = order
        .into_iter()
        .map(|project| {
            let total_tokens = totals[&project];
            ProjectBar { project, total_tokens }
        })
        .collect();
    bars.sort_by_key(|b| std::cmp::Reverse(b.total_tokens));
    bars
}

/// Total tokens per model (Opus/Sonnet/Haiku/Fable) across all sessions,
/// sorted descending. Unlike [`build_project_totals`], this re-parses every
/// session's transcript (via [`analyze_session`]) because the per-model split
/// isn't captured by the lightweight discovery pass — a session's total may
/// mix models, so attributing it to a single "primary" model would misreport
/// the split.
pub fn build_model_totals(sessions: &[SessionResult]) -> Vec<ModelBar> {
    let mut opus = 0i64;
    let mut sonnet = 0i64;
    let mut haiku = 0i64;
    let mut fable = 0i64;

    for session in sessions {
        let entries = parse_transcript_file(&session.path);
        let analysis = analyze_session(&entries);
        opus += analysis.opus_tokens;
        sonnet += analysis.sonnet_tokens;
        haiku += analysis.haiku_tokens;
        fable += analysis.fable_tokens;
    }

    let mut bars: Vec<ModelBar> = [
        ("Opus", opus),
        ("Sonnet", sonnet),
        ("Haiku", haiku),
        ("Fable", fable),
    ]
    .into_iter()
    .filter(|(_, total_tokens)| *total_tokens > 0)
    .map(|(model, total_tokens)| ModelBar { model: model.to_string(), total_tokens })
    .collect();
    bars.sort_by_key(|b| std::cmp::Reverse(b.total_tokens));
    bars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(date: &str, project: &str, tokens: i64, mtime: i64) -> SessionResult {
        SessionResult {
            session_id: "s".to_string(),
            path: PathBuf::from("/x.jsonl"),
            date: date.to_string(),
            tokens,
            project: project.to_string(),
            mtime,
            title: None,
        }
    }

    #[test]
    fn bar_chart_empty() {
        assert_eq!(build_bar_chart_data(&[]).days.len(), 0);
    }

    #[test]
    fn bar_chart_groups_and_sorts() {
        let sessions = [
            session("2025-06-16", "-home-code-alpha", 100, 2),
            session("2025-06-15", "-home-code-alpha", 50, 1),
            session("2025-06-15", "-home-code-beta", 200, 1),
            session("2025-06-15", "-home-code-alpha", 25, 1),
        ];
        let data = build_bar_chart_data(&sessions);
        // Dates ascending.
        assert_eq!(data.days.len(), 2);
        assert_eq!(data.days[0].date, "2025-06-15");
        assert_eq!(data.days[1].date, "2025-06-16");
        // On the first day, beta (200) outranks alpha (50+25=75).
        assert_eq!(data.days[0].projects[0].project, "beta");
        assert_eq!(data.days[0].projects[0].total_tokens, 200);
        assert_eq!(data.days[0].projects[1].project, "alpha");
        assert_eq!(data.days[0].projects[1].total_tokens, 75);
    }

    #[test]
    fn find_recent_reads_dir_and_counts_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-home-code-demo");
        std::fs::create_dir(&proj).unwrap();
        std::fs::write(
            proj.join("abc123.jsonl"),
            "{\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n",
        )
        .unwrap();
        // A non-jsonl file is ignored.
        std::fs::write(proj.join("notes.txt"), "ignore me").unwrap();

        let sessions = find_recent_sessions(tmp.path(), 500, 0.0, 1_700_000_000_000).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "abc123");
        assert_eq!(sessions[0].tokens, 15);
        assert_eq!(sessions[0].project, "-home-code-demo");
    }

    #[test]
    fn find_session_path_locates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-home-code-demo");
        std::fs::create_dir(&proj).unwrap();
        let file = proj.join("wanted.jsonl");
        std::fs::write(&file, "{}").unwrap();

        let found = find_session_path(tmp.path(), "wanted").unwrap();
        assert_eq!(found, Some(file));
        assert_eq!(find_session_path(tmp.path(), "missing").unwrap(), None);
    }

    #[test]
    fn project_totals_sums_across_dates_and_sorts() {
        let sessions = [
            session("2025-06-16", "-home-code-alpha", 100, 2),
            session("2025-06-15", "-home-code-alpha", 50, 1),
            session("2025-06-15", "-home-code-beta", 200, 1),
        ];
        let bars = build_project_totals(&sessions);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].project, "beta");
        assert_eq!(bars[0].total_tokens, 200);
        assert_eq!(bars[1].project, "alpha");
        assert_eq!(bars[1].total_tokens, 150);
    }

    #[test]
    fn project_totals_empty() {
        assert!(build_project_totals(&[]).is_empty());
    }

    fn write_session(
        tmp: &std::path::Path,
        name: &str,
        model: &str,
        input: i64,
        output: i64,
    ) -> SessionResult {
        let path = tmp.join(name);
        std::fs::write(
            &path,
            format!(
                "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"model\":\"{model}\",\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output}}}}}}}\n"
            ),
        )
        .unwrap();
        SessionResult {
            session_id: name.trim_end_matches(".jsonl").to_string(),
            path,
            date: "2025-06-15".to_string(),
            tokens: input + output,
            project: "-home-code-demo".to_string(),
            mtime: 1,
            title: None,
        }
    }

    #[test]
    fn model_totals_sums_per_model_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = [
            write_session(tmp.path(), "s1.jsonl", "claude-opus-4", 100, 50),
            write_session(tmp.path(), "s2.jsonl", "claude-sonnet-4", 300, 100),
            write_session(tmp.path(), "s3.jsonl", "claude-sonnet-4", 50, 20),
        ];
        let bars = build_model_totals(&sessions);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].model, "Sonnet");
        assert_eq!(bars[0].total_tokens, 470);
        assert_eq!(bars[1].model, "Opus");
        assert_eq!(bars[1].total_tokens, 150);
    }

    #[test]
    fn model_totals_empty() {
        assert!(build_model_totals(&[]).is_empty());
    }

    #[test]
    fn model_totals_includes_fable() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = [
            write_session(tmp.path(), "s1.jsonl", "claude-opus-4", 100, 50),
            write_session(tmp.path(), "s2.jsonl", "claude-fable-5", 300, 100),
            write_session(tmp.path(), "s3.jsonl", "claude-fable-5[1m]", 50, 20),
        ];
        let bars = build_model_totals(&sessions);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].model, "Fable");
        assert_eq!(bars[0].total_tokens, 470);
        assert_eq!(bars[1].model, "Opus");
        assert_eq!(bars[1].total_tokens, 150);
    }
}
