//! Ranked waste/habit findings over a scanned session window — the app's
//! Insights tab.
//!
//! Answer-first by design: instead of an explorer the user has to hunt
//! through, each finding names one session, one number, and why it matters.
//! Everything derives from the ledger's cached single-parse scan
//! ([`crate::ledger::SessionDetail`]) — no second parse pass.
//!
//! Thresholds are deliberately conservative: a finding should mean "worth a
//! look", not restate that some sessions are bigger than others. `score` is
//! "multiples of the flagging threshold", so findings of different kinds sort
//! against each other sanely.

use serde::Serialize;

use crate::ledger::SessionDetail;

/// A session must be ≥ this multiple of the window median (and past the
/// absolute floor) to be flagged as a token outlier.
const OUTLIER_FACTOR: f64 = 5.0;
/// Ignore outliers below this in+out volume — 5× a tiny median is noise.
const OUTLIER_FLOOR: i64 = 200_000;
/// Sessions below this volume are excluded from the median: one-question
/// sessions are common and would drag the baseline to a few K, making every
/// real work session an "outlier" at absurd multiples. Falls back to the
/// all-sessions median when nothing clears the bar.
const MEDIAN_MIN: i64 = 20_000;
/// At most this many findings of one kind, so a window where one pattern
/// fires everywhere still shows the other patterns.
const MAX_PER_KIND: usize = 4;
/// Extra reads of already-read files before a session is a re-read loop.
const REREAD_MIN: i64 = 15;
/// Cache-write tokens must exceed this floor to flag cache churn…
const CACHE_CHURN_FLOOR: i64 = 2_000_000;
/// …and be at least this multiple of the session's in+out volume.
const CACHE_CHURN_FACTOR: f64 = 20.0;
/// Human prompts in one session before it's flagged as a marathon.
const MARATHON_TURNS: i64 = 50;
/// Findings returned at most — a feed, not a firehose.
const MAX_INSIGHTS: usize = 12;

/// What a finding is about. Serialized as a camelCase discriminant the
/// frontend matches on for icon/color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InsightKind {
    /// In+out volume far above the window median.
    TokenOutlier,
    /// The same files were re-read many times — a context-loss loop.
    RereadLoop,
    /// Cache writes dwarf the useful work — context re-established over and
    /// over (restarts, compactions, very long sessions).
    CacheChurn,
    /// Very many human prompts in a single session.
    Marathon,
}

/// One ranked finding, referencing a session by index into the scanned slice.
#[derive(Debug, Clone, PartialEq)]
pub struct Insight {
    pub kind: InsightKind,
    /// Multiples of the flagging threshold — the cross-kind sort key.
    pub score: f64,
    /// Index into the `details` slice passed to [`build_insights`].
    pub index: usize,
    /// Headline number, e.g. `"6.2× median"` or `"38 re-reads"`.
    pub metric: String,
    /// One-sentence "why this matters".
    pub detail: String,
}

fn fmt_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Median in+out volume across substantial sessions (≥ [`MEDIAN_MIN`]),
/// falling back to all nonzero sessions when none qualify.
fn median_billable(details: &[SessionDetail]) -> i64 {
    let mut volumes: Vec<i64> =
        details.iter().map(|d| d.billable()).filter(|v| *v >= MEDIAN_MIN).collect();
    if volumes.is_empty() {
        volumes = details.iter().map(|d| d.billable()).filter(|v| *v > 0).collect();
    }
    if volumes.is_empty() {
        return 0;
    }
    volumes.sort_unstable();
    volumes[volumes.len() / 2]
}

/// Compute the ranked findings for a scanned window, highest score first.
pub fn build_insights(details: &[SessionDetail]) -> Vec<Insight> {
    let median = median_billable(details);
    let mut insights: Vec<Insight> = Vec::new();

    for (index, d) in details.iter().enumerate() {
        let billable = d.billable();

        if median > 0 {
            let factor = billable as f64 / median as f64;
            if factor >= OUTLIER_FACTOR && billable >= OUTLIER_FLOOR {
                insights.push(Insight {
                    kind: InsightKind::TokenOutlier,
                    score: factor / OUTLIER_FACTOR,
                    index,
                    metric: format!("{factor:.1}× median"),
                    detail: format!(
                        "{} in+out vs a {} median — worth knowing what made this one so heavy.",
                        fmt_tokens(billable),
                        fmt_tokens(median)
                    ),
                });
            }
        }

        if d.repeated_reads >= REREAD_MIN {
            insights.push(Insight {
                kind: InsightKind::RereadLoop,
                score: d.repeated_reads as f64 / REREAD_MIN as f64,
                index,
                metric: format!("{} re-reads", d.repeated_reads),
                detail: format!(
                    "Read already-read files {} extra times — context was getting lost; \
                     smaller tasks or a CLAUDE.md note may help.",
                    d.repeated_reads
                ),
            });
        }

        let cache_write = d.usage.cache_creation_tokens;
        if cache_write >= CACHE_CHURN_FLOOR
            && billable > 0
            && cache_write as f64 >= CACHE_CHURN_FACTOR * billable as f64
        {
            let ratio = cache_write as f64 / billable as f64;
            insights.push(Insight {
                kind: InsightKind::CacheChurn,
                score: ratio / CACHE_CHURN_FACTOR,
                index,
                metric: format!("{} cache written", fmt_tokens(cache_write)),
                detail: format!(
                    "{} cache-write tokens against {} of work ({:.0}×) — context was \
                     re-established over and over.",
                    fmt_tokens(cache_write),
                    fmt_tokens(billable),
                    ratio
                ),
            });
        }

        if d.user_turns >= MARATHON_TURNS {
            insights.push(Insight {
                kind: InsightKind::Marathon,
                score: d.user_turns as f64 / MARATHON_TURNS as f64,
                index,
                metric: format!("{} prompts", d.user_turns),
                detail: format!(
                    "{} human prompts in one session — long sessions accumulate context \
                     cost; splitting work keeps each one cheap.",
                    d.user_turns
                ),
            });
        }
    }

    insights.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    // Keep the feed mixed: at most MAX_PER_KIND of any one kind, best-first.
    let mut kind_counts: std::collections::HashMap<InsightKind, usize> = Default::default();
    insights.retain(|i| {
        let count = kind_counts.entry(i.kind).or_insert(0);
        *count += 1;
        *count <= MAX_PER_KIND
    });
    insights.truncate(MAX_INSIGHTS);
    insights
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tt_claude_code::UsageTotals;

    fn detail(input: i64, output: i64) -> SessionDetail {
        SessionDetail {
            session_id: "s".into(),
            path: PathBuf::from("/x.jsonl"),
            project: "demo".into(),
            date: "2026-07-17".into(),
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
            user_turns: 0,
            prompt_blob: String::new(),
        }
    }

    #[test]
    fn empty_and_quiet_windows_produce_no_findings() {
        assert!(build_insights(&[]).is_empty());
        let quiet = vec![detail(10_000, 5_000); 5];
        assert!(build_insights(&quiet).is_empty());
    }

    #[test]
    fn token_outlier_needs_factor_and_floor() {
        // Median 30k; 90k is only 3× — not flagged.
        let mut details = vec![detail(20_000, 10_000); 4];
        details.push(detail(60_000, 30_000));
        assert!(build_insights(&details).is_empty());

        // 1.5M against a 30k median clears both bars.
        let mut details = vec![detail(20_000, 10_000); 4];
        details.push(detail(1_000_000, 500_000));
        let insights = build_insights(&details);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, InsightKind::TokenOutlier);
        assert_eq!(insights[0].index, 4);
        assert!(insights[0].metric.contains("× median"), "{}", insights[0].metric);
    }

    #[test]
    fn median_ignores_trivial_sessions() {
        // 40 one-question sessions (1.5k) must not drag the median down and
        // make a normal 300k work session an outlier.
        let mut details = vec![detail(1_000, 500); 40];
        details.push(detail(150_000, 50_000));
        details.push(detail(200_000, 100_000));
        assert!(build_insights(&details).is_empty());
    }

    #[test]
    fn reread_loop_flagged_at_threshold() {
        let mut d = detail(10_000, 5_000);
        d.repeated_reads = REREAD_MIN;
        let insights = build_insights(&[d]);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, InsightKind::RereadLoop);
        assert_eq!(insights[0].score, 1.0);
        assert_eq!(insights[0].metric, "15 re-reads");
    }

    #[test]
    fn cache_churn_needs_floor_and_ratio() {
        // Big cache write but healthy ratio (large billable) — not churn.
        let mut d = detail(1_000_000, 500_000);
        d.usage.cache_creation_tokens = 3_000_000;
        assert!(build_insights(&[d]).is_empty());

        // Small work, huge cache write — churn.
        let mut d = detail(50_000, 30_000);
        d.usage.cache_creation_tokens = 3_000_000;
        let insights = build_insights(&[d]);
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].kind, InsightKind::CacheChurn);
    }

    #[test]
    fn marathon_flagged_and_ranked_by_score() {
        let mut long = detail(10_000, 5_000);
        long.user_turns = 100; // score 2.0
        let mut reread = detail(10_000, 5_000);
        reread.repeated_reads = 45; // score 3.0
        let insights = build_insights(&[long, reread]);
        assert_eq!(insights.len(), 2);
        assert_eq!(insights[0].kind, InsightKind::RereadLoop);
        assert_eq!(insights[1].kind, InsightKind::Marathon);
    }

    #[test]
    fn one_kind_cannot_monopolize_the_feed() {
        let mut details = Vec::new();
        for _ in 0..20 {
            let mut d = detail(10_000, 5_000);
            d.repeated_reads = 30;
            details.push(d);
        }
        // 20 re-read findings collapse to the per-kind cap.
        assert_eq!(build_insights(&details).len(), MAX_PER_KIND);

        // A weaker finding of another kind still makes the feed.
        let mut marathon = detail(10_000, 5_000);
        marathon.user_turns = MARATHON_TURNS;
        details.push(marathon);
        let insights = build_insights(&details);
        assert_eq!(insights.len(), MAX_PER_KIND + 1);
        assert!(insights.iter().any(|i| i.kind == InsightKind::Marathon));
    }
}
