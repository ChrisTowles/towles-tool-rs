//! Single-parse session scan + aggregation + search, backing the app's
//! Claude Sessions screen ("where did the tokens go, and which sessions are
//! the outliers"). Unlike the `find_recent_sessions` + `build_model_totals`
//! pair — which parses every transcript twice — this scans once per file and
//! keeps everything the screen needs: usage components, per-model split,
//! title, and the human prompt text as a search corpus.

use std::path::{Path, PathBuf};

use tt_claude_code::{
    UsageTotals, parse_transcript_file, session_cwd, session_title, usage_totals, user_prompt_blob,
    user_prompts,
};

use crate::Result;
use crate::analyzer::{analyze_session, extract_project_name};
use crate::parser::calculate_cutoff_ms;
use crate::types::{BarChartDay, ModelBar, ProjectBar};

/// Per-session cap on the searchable prompt text, so a 500-session scan stays
/// bounded in memory (≤ ~8 MB of corpus).
const PROMPT_BLOB_MAX_BYTES: usize = 16 * 1024;

/// Everything the Ledger knows about one session, from a single parse.
#[derive(Debug, Clone)]
pub struct SessionDetail {
    pub session_id: String,
    pub path: PathBuf,
    /// Normalized repo name: slot clones and worktrees fold into their repo.
    pub project: String,
    /// `YYYY-MM-DD` (local) of the file's mtime.
    pub date: String,
    pub mtime: i64,
    pub title: Option<String>,
    /// The session's real launch directory (from the transcript's `cwd`
    /// field), for "open this session in Agentboard" — `None` for older
    /// transcripts that predate the field.
    pub cwd: Option<String>,
    pub usage: UsageTotals,
    pub opus_tokens: i64,
    pub sonnet_tokens: i64,
    pub haiku_tokens: i64,
    pub fable_tokens: i64,
    /// Extra reads of files already read this session (a context-loss smell).
    pub repeated_reads: i64,
    /// Estimated USD cost, priced per model (see [`crate::pricing`]).
    pub cost_usd: f64,
    /// Number of human prompts in the session.
    pub user_turns: i64,
    /// Human prompt text, newline-joined, capped — the search corpus.
    pub prompt_blob: String,
}

impl SessionDetail {
    /// Input + output tokens — the activity volume the Ledger charts.
    pub fn billable(&self) -> i64 {
        self.usage.billable()
    }
}

/// Repo name with parallel-checkout suffixes folded away: a trailing
/// `-slot-<n>` and anything from `-claude-worktrees` on. The Ledger compares
/// repos, and one repo checked out five ways is still one repo.
pub fn normalize_repo_name(encoded_project: &str) -> String {
    // Worktree dirs encode as `<repo>--claude-worktrees-<name>`; the doubled
    // `-` means the marker survives as `-claude-worktrees` after splitting.
    let base = match encoded_project.find("-claude-worktrees") {
        Some(idx) => &encoded_project[..idx],
        None => encoded_project,
    };
    let name = extract_project_name(base.trim_end_matches('-'));
    // `-slot-<n>` may sit mid-name when the session's cwd was a subdirectory
    // of the slot (`…-slot-0-crates-tauri-tt-app`); everything from the marker
    // on is checkout/subpath noise, not repo identity.
    match name.find("-slot-") {
        Some(idx) if name[idx + 6..].starts_with(|c: char| c.is_ascii_digit()) => {
            name[..idx].to_string()
        }
        _ => name,
    }
}

/// Scan `projects_dir` for sessions touched in the last `days` (0 = all),
/// newest first, capped at `limit`, parsing each survivor's transcript once.
pub fn scan_sessions_detailed(
    projects_dir: &Path,
    limit: usize,
    days: f64,
    now_ms: i64,
) -> Result<Vec<SessionDetail>> {
    let cutoff_ms = calculate_cutoff_ms(days, now_ms);

    // Cheap pass: paths + mtimes only, so the limit applies before any parse.
    let mut candidates: Vec<(i64, String, String, PathBuf)> = Vec::new();
    for project_entry in std::fs::read_dir(projects_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let encoded = project_entry.file_name().to_string_lossy().to_string();
        for file_entry in std::fs::read_dir(&project_path)? {
            let file_entry = file_entry?;
            let name = file_entry.file_name().to_string_lossy().to_string();
            let Some(session_id) = name.strip_suffix(".jsonl") else {
                continue;
            };
            let meta = file_entry.metadata()?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if cutoff_ms > 0 && mtime < cutoff_ms {
                continue;
            }
            candidates.push((mtime, session_id.to_string(), encoded.clone(), file_entry.path()));
        }
    }
    candidates.sort_by_key(|(mtime, ..)| std::cmp::Reverse(*mtime));
    candidates.truncate(limit);

    let mut details = Vec::with_capacity(candidates.len());
    for (mtime, session_id, encoded, path) in candidates {
        let entries = parse_transcript_file(&path);
        let analysis = analyze_session(&entries);
        // Human prompts only — the transcript's `user` lines are mostly
        // machine noise (tool results, envelopes), which must not count.
        let user_turns = user_prompts(&entries).len() as i64;
        details.push(SessionDetail {
            session_id,
            project: normalize_repo_name(&encoded),
            date: local_date(mtime),
            mtime,
            title: session_title(&entries),
            cwd: session_cwd(&entries),
            usage: usage_totals(&entries),
            opus_tokens: analysis.opus_tokens,
            sonnet_tokens: analysis.sonnet_tokens,
            haiku_tokens: analysis.haiku_tokens,
            fable_tokens: analysis.fable_tokens,
            repeated_reads: analysis.repeated_reads,
            cost_usd: analysis.cost_usd,
            user_turns,
            prompt_blob: user_prompt_blob(&entries, PROMPT_BLOB_MAX_BYTES),
            path,
        });
    }
    Ok(details)
}

fn local_date(mtime_ms: i64) -> String {
    use chrono::{DateTime, Local};
    let dt: DateTime<Local> = DateTime::from(
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms.max(0) as u64),
    );
    dt.format("%Y-%m-%d").to_string()
}

/// Whole-window totals for the stat tiles.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerTotals {
    pub sessions: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// Estimated USD cost across the window (see [`crate::pricing`]).
    pub cost_usd: f64,
}

pub fn ledger_totals(details: &[SessionDetail]) -> LedgerTotals {
    let mut t = LedgerTotals { sessions: details.len() as i64, ..Default::default() };
    for d in details {
        t.input_tokens += d.usage.input_tokens;
        t.output_tokens += d.usage.output_tokens;
        t.cache_read_tokens += d.usage.cache_read_tokens;
        t.cache_creation_tokens += d.usage.cache_creation_tokens;
        t.cost_usd += d.cost_usd;
    }
    t
}

/// Per-day per-repo billable tokens, dates ascending, repos descending by
/// tokens within a day. Same shape the treemap HTML's bar chart consumes.
pub fn build_ledger_days(details: &[SessionDetail]) -> Vec<BarChartDay> {
    let mut by_date: std::collections::BTreeMap<String, Vec<(String, i64)>> = Default::default();
    for d in details {
        let projects = by_date.entry(d.date.clone()).or_default();
        match projects.iter_mut().find(|(p, _)| *p == d.project) {
            Some(existing) => existing.1 += d.billable(),
            None => projects.push((d.project.clone(), d.billable())),
        }
    }
    by_date
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
        .collect()
}

/// Billable tokens per repo, descending.
pub fn build_ledger_project_totals(details: &[SessionDetail]) -> Vec<ProjectBar> {
    let mut totals: Vec<(String, i64)> = Vec::new();
    for d in details {
        match totals.iter_mut().find(|(p, _)| *p == d.project) {
            Some(existing) => existing.1 += d.billable(),
            None => totals.push((d.project.clone(), d.billable())),
        }
    }
    totals.sort_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
    totals.into_iter().map(|(project, total_tokens)| ProjectBar { project, total_tokens }).collect()
}

/// Billable tokens per model family, descending — no re-parse.
pub fn build_ledger_model_totals(details: &[SessionDetail]) -> Vec<ModelBar> {
    let sums = details.iter().fold([0i64; 4], |mut acc, d| {
        acc[0] += d.opus_tokens;
        acc[1] += d.sonnet_tokens;
        acc[2] += d.haiku_tokens;
        acc[3] += d.fable_tokens;
        acc
    });
    let mut bars: Vec<ModelBar> = [("Opus", 0), ("Sonnet", 1), ("Haiku", 2), ("Fable", 3)]
        .into_iter()
        .filter(|(_, i)| sums[*i] > 0)
        .map(|(model, i)| ModelBar { model: model.to_string(), total_tokens: sums[i] })
        .collect();
    bars.sort_by_key(|b| std::cmp::Reverse(b.total_tokens));
    bars
}

/// One search hit: the matching session's index into the scanned slice, plus a
/// context snippet when the match was in prompt text (a title match needs no
/// snippet — the title is already displayed).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub index: usize,
    pub snippet: Option<String>,
}

/// Case-insensitive substring search over title + prompt corpus. Hits keep the
/// input order (callers pass details newest-first).
pub fn search_sessions(details: &[SessionDetail], query: &str) -> Vec<SearchHit> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (index, d) in details.iter().enumerate() {
        if d.title.as_deref().is_some_and(|t| t.to_lowercase().contains(&q)) {
            hits.push(SearchHit { index, snippet: None });
        } else if let Some(pos) = d.prompt_blob.to_lowercase().find(&q) {
            hits.push(SearchHit {
                index,
                snippet: Some(snippet_around(&d.prompt_blob, pos, q.len())),
            });
        }
    }
    hits
}

/// ~120 chars of context around a byte match, on char boundaries, with
/// newlines flattened and ellipses marking truncation.
fn snippet_around(text: &str, match_pos: usize, match_len: usize) -> String {
    let mut start = match_pos.saturating_sub(40);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (match_pos + match_len + 80).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let mut s = text[start..end].replace(['\n', '\r'], " ").trim().to_string();
    if start > 0 {
        s = format!("…{s}");
    }
    if end < text.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(project: &str, date: &str, input: i64, output: i64) -> SessionDetail {
        SessionDetail {
            session_id: "s".into(),
            path: PathBuf::from("/x.jsonl"),
            project: project.into(),
            date: date.into(),
            mtime: 0,
            title: None,
            cwd: None,
            usage: UsageTotals {
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
            opus_tokens: 0,
            sonnet_tokens: 0,
            haiku_tokens: 0,
            fable_tokens: input + output,
            repeated_reads: 0,
            cost_usd: 0.0,
            user_turns: 0,
            prompt_blob: String::new(),
        }
    }

    #[test]
    fn normalize_folds_slots_and_worktrees() {
        assert_eq!(
            normalize_repo_name("-home-ctowles-code-p-towles-tool-repos-towles-tool-rs-slot-2"),
            "towles-tool-rs"
        );
        assert_eq!(
            normalize_repo_name(
                "-home-ctowles-code-p-blog-repos-blog-primary--claude-worktrees-friendly-tereshkova-ea358d"
            ),
            "blog-primary"
        );
        assert_eq!(normalize_repo_name("-home-ctowles-code-p-dotfiles"), "dotfiles");
        // A session whose cwd was a subdirectory of a slot still folds.
        assert_eq!(
            normalize_repo_name(
                "-home-ctowles-code-p-towles-tool-repos-towles-tool-rs-slot-0-crates-tauri-tt-app"
            ),
            "towles-tool-rs"
        );
        // A non-numeric "slot" suffix is left alone.
        assert_eq!(normalize_repo_name("-home-u-code-time-slot-machine"), "time-slot-machine");
    }

    #[test]
    fn days_group_by_date_then_repo() {
        let details = [
            detail("alpha", "2026-07-10", 100, 0),
            detail("beta", "2026-07-10", 300, 0),
            detail("alpha", "2026-07-10", 50, 0),
            detail("alpha", "2026-07-11", 10, 5),
        ];
        let days = build_ledger_days(&details);
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].date, "2026-07-10");
        assert_eq!(days[0].projects[0].project, "beta");
        assert_eq!(days[0].projects[1].total_tokens, 150);
        assert_eq!(days[1].projects[0].total_tokens, 15);
    }

    #[test]
    fn totals_sum_components() {
        let mut d = detail("a", "2026-07-10", 10, 20);
        d.usage.cache_read_tokens = 1000;
        d.usage.cache_creation_tokens = 30;
        d.cost_usd = 0.5;
        let t = ledger_totals(&[d.clone(), d]);
        assert_eq!(t.sessions, 2);
        assert_eq!(t.input_tokens, 20);
        assert_eq!(t.output_tokens, 40);
        assert_eq!(t.cache_read_tokens, 2000);
        assert_eq!(t.cache_creation_tokens, 60);
        assert!((t.cost_usd - 1.0).abs() < 1e-9);
    }

    #[test]
    fn model_totals_from_details_without_reparse() {
        let bars = build_ledger_model_totals(&[detail("a", "d", 5, 5)]);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].model, "Fable");
        assert_eq!(bars[0].total_tokens, 10);
    }

    #[test]
    fn search_matches_title_and_prompts() {
        let mut a = detail("a", "d", 1, 1);
        a.title = Some("Fix the Ledger screen".into());
        let mut b = detail("b", "d", 1, 1);
        b.prompt_blob = "let's talk about echarts stacked bars\nand tooltips".into();
        let details = [a, b, detail("c", "d", 1, 1)];

        let hits = search_sessions(&details, "LEDGER");
        assert_eq!(hits, vec![SearchHit { index: 0, snippet: None }]);

        let hits = search_sessions(&details, "stacked bars");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].index, 1);
        let snip = hits[0].snippet.as_deref().unwrap();
        assert!(snip.contains("stacked bars"), "snippet was {snip:?}");
        assert!(!snip.contains('\n'));

        assert!(search_sessions(&details, "   ").is_empty());
        assert!(search_sessions(&details, "nomatch").is_empty());
    }

    #[test]
    fn snippet_handles_multibyte_boundaries() {
        let text = "ééééééééééééééééééééééééé needle after";
        let pos = text.find("needle").unwrap();
        let s = snippet_around(text, pos, 6);
        assert!(s.contains("needle"));
    }

    #[test]
    fn scan_parses_once_and_fills_detail() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-home-u-code-demo-slot-1");
        std::fs::create_dir(&proj).unwrap();
        std::fs::write(
            proj.join("abc.jsonl"),
            concat!(
                "{\"type\":\"assistant\",\"cwd\":\"/home/u/code/demo\",\"message\":{\"role\":\"assistant\",\"model\":\"claude-fable-5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_input_tokens\":100}}}\n",
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hello ledger\"}}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Demo session\"}\n",
            ),
        )
        .unwrap();

        let details = scan_sessions_detailed(tmp.path(), 10, 0.0, 1_700_000_000_000).unwrap();
        assert_eq!(details.len(), 1);
        let d = &details[0];
        assert_eq!(d.session_id, "abc");
        assert_eq!(d.project, "demo");
        assert_eq!(d.title.as_deref(), Some("Demo session"));
        assert_eq!(d.cwd.as_deref(), Some("/home/u/code/demo"));
        assert_eq!(d.billable(), 15);
        assert_eq!(d.usage.cache_read_tokens, 100);
        assert_eq!(d.fable_tokens, 15);
        // Fable: (10*10 + 5*50 + 100*1.0) / 1e6
        assert!((d.cost_usd - 0.00045).abs() < 1e-9);
        assert_eq!(d.prompt_blob, "hello ledger");
    }
}
