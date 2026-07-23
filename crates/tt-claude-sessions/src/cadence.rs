//! Human-prompt cadence: when in the day, and how often, the user actually
//! types — as distinct from every other view in this crate, which is about
//! token/cost accounting. Derives from [`SessionDetail::prompt_times_ms`],
//! already collected by the ledger's single-parse scan — no re-parse.

use std::collections::BTreeMap;
use std::time::{Duration, UNIX_EPOCH};

use chrono::{DateTime, Local, Timelike};
use serde::Serialize;

use crate::ledger::SessionDetail;

/// Prompt count for one local calendar day.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayBucket {
    pub date: String,
    pub count: i64,
}

/// Prompt count for one (local calendar day, local hour) pair — one cell of
/// the day×hour grid. Sparse: only nonzero cells are included.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayHourCell {
    pub date: String,
    pub hour: u8,
    pub count: i64,
}

/// Prompt-cadence summary over a scanned window: how many human prompts land
/// on each calendar day, and the day×hour breakdown for that grid view.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CadenceSummary {
    /// Calendar days that had at least one prompt, ascending by date.
    pub by_day: Vec<DayBucket>,
    /// Nonzero (day, hour) cells, sorted by date then hour.
    pub by_day_hour: Vec<DayHourCell>,
    /// Prompts with a parseable timestamp, across all sessions scanned.
    pub total_prompts: i64,
}

fn local_datetime(ms: i64) -> DateTime<Local> {
    DateTime::from(UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64))
}

/// Bucket every session's prompt timestamps by local calendar day and by
/// (local calendar day, local hour) pair.
///
/// `cutoff_ms` (from [`crate::calculate_cutoff_ms`], `0` = no filtering) is
/// applied per-*prompt*, not just per-session: `details` was already
/// filtered by session mtime, which is not the same window. A long-running
/// or resumed session can be touched (and pass the mtime filter) today while
/// carrying prompt timestamps from months earlier — without this second
/// filter, a "last 30 days" cadence view would silently include prompts from
/// well outside that window.
pub fn build_cadence(details: &[SessionDetail], cutoff_ms: i64) -> CadenceSummary {
    let mut by_day: BTreeMap<String, i64> = BTreeMap::new();
    let mut by_day_hour: BTreeMap<(String, u8), i64> = BTreeMap::new();
    let mut total_prompts = 0i64;

    for d in details {
        for &ms in &d.prompt_times_ms {
            if cutoff_ms > 0 && ms < cutoff_ms {
                continue;
            }
            let dt = local_datetime(ms);
            let date = dt.format("%Y-%m-%d").to_string();
            *by_day.entry(date.clone()).or_insert(0) += 1;
            *by_day_hour.entry((date, dt.hour() as u8)).or_insert(0) += 1;
            total_prompts += 1;
        }
    }

    CadenceSummary {
        by_day: by_day.into_iter().map(|(date, count)| DayBucket { date, count }).collect(),
        by_day_hour: by_day_hour
            .into_iter()
            .map(|((date, hour), count)| DayHourCell { date, hour, count })
            .collect(),
        total_prompts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tt_claude_code::UsageTotals;

    fn detail(prompt_times_ms: Vec<i64>) -> SessionDetail {
        SessionDetail {
            session_id: "s".into(),
            path: PathBuf::from("/x.jsonl"),
            project: "demo".into(),
            date: "2026-07-17".into(),
            mtime: 0,
            title: None,
            cwd: None,
            usage: UsageTotals::default(),
            opus_tokens: 0,
            sonnet_tokens: 0,
            haiku_tokens: 0,
            fable_tokens: 0,
            repeated_reads: 0,
            cost_usd: 0.0,
            user_turns: prompt_times_ms.len() as i64,
            prompt_blob: String::new(),
            prompt_times_ms,
        }
    }

    #[test]
    fn empty_window_is_all_zeros() {
        let summary = build_cadence(&[], 0);
        assert!(summary.by_day.is_empty());
        assert!(summary.by_day_hour.is_empty());
        assert_eq!(summary.total_prompts, 0);
    }

    #[test]
    fn buckets_by_local_day_and_hour() {
        // 2026-07-11T19:13:52.831Z and one hour later, same local day.
        let details = [detail(vec![
            1_783_797_232_831,
            1_783_797_232_831 + 3_600_000,
        ])];
        let summary = build_cadence(&details, 0);
        assert_eq!(summary.total_prompts, 2);
        assert_eq!(summary.by_day.len(), 1);
        assert_eq!(summary.by_day[0].count, 2);

        // Two distinct hours on the same day, each counted once.
        assert_eq!(summary.by_day_hour.len(), 2);
        assert_eq!(summary.by_day_hour[0].date, summary.by_day[0].date);
        assert!(summary.by_day_hour.iter().all(|c| c.count == 1));
        assert_ne!(summary.by_day_hour[0].hour, summary.by_day_hour[1].hour);
    }

    #[test]
    fn sums_across_sessions_and_sorts_days_ascending() {
        let later = detail(vec![1_783_797_232_831 + 86_400_000]); // +1 day
        let earlier = detail(vec![1_783_797_232_831]);
        let summary = build_cadence(&[later, earlier], 0);
        assert_eq!(summary.total_prompts, 2);
        assert_eq!(summary.by_day.len(), 2);
        assert!(summary.by_day[0].date < summary.by_day[1].date);
    }

    #[test]
    fn cutoff_filters_per_prompt_not_just_per_session() {
        // A session touched "recently" (it passed the mtime filter to make it
        // into `details` at all) can still carry a much older prompt — a
        // resumed long-running conversation. The cutoff must drop that
        // individual prompt, not count it just because its session survived.
        let old_ms = 1_783_797_232_831;
        let recent_ms = old_ms + 90 * 86_400_000; // +90 days
        let d = detail(vec![old_ms, recent_ms]);
        let summary = build_cadence(&[d], recent_ms - 30 * 86_400_000);
        assert_eq!(summary.total_prompts, 1);
        assert_eq!(summary.by_day.len(), 1);
    }
}
