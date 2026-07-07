//! Flat session-row output as JSON/CSV. Ports `src/commands/graph/format.ts`.

use serde::Serialize;

use crate::Result;
use crate::analyzer::{SessionAnalysis, analyze_session, extract_project_name, get_primary_model};
use crate::types::SessionResult;
use tt_claude_code::parse_transcript_file;

/// A single flat session row. Serializes with camelCase keys.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub session_path: String,
    pub project: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cost: f64,
    pub date: String,
}

/// Approximate pricing per million tokens (as of 2025). `(input, output)`.
fn cost_per_million(model: &str) -> Option<(f64, f64)> {
    match model {
        "opus" => Some((15.0, 75.0)),
        "sonnet" => Some((3.0, 15.0)),
        "haiku" => Some((0.8, 4.0)),
        _ => None,
    }
}

/// Estimate cost by distributing input/output tokens proportionally across the
/// models actually used, rounded to 4 decimal places. Ports `estimateCost`.
///
/// Every model that ran counts toward the denominator, including Fable — even
/// though `cost_per_million` has no Fable price yet (its share is effectively
/// priced at 0). Omitting it would misattribute a mixed session's whole
/// input/output token spend to the priced models that happen to appear.
fn estimate_cost(analysis: &SessionAnalysis) -> f64 {
    let total = analysis.opus_tokens
        + analysis.sonnet_tokens
        + analysis.haiku_tokens
        + analysis.fable_tokens;
    if total == 0 {
        return 0.0;
    }

    let mut cost = 0.0;
    for (model, tokens) in [
        ("opus", analysis.opus_tokens),
        ("sonnet", analysis.sonnet_tokens),
        ("haiku", analysis.haiku_tokens),
        ("fable", analysis.fable_tokens),
    ] {
        if tokens == 0 {
            continue;
        }
        // A model with no price in the table dilutes the fractions but adds no
        // cost of its own (currently Fable). It must still count in `total`.
        let Some((in_rate, out_rate)) = cost_per_million(model) else {
            continue;
        };
        let fraction = tokens as f64 / total as f64;
        let input_share = analysis.input_tokens as f64 * fraction;
        let output_share = analysis.output_tokens as f64 * fraction;
        cost += (input_share * in_rate + output_share * out_rate) / 1_000_000.0;
    }

    (cost * 10000.0).round() / 10000.0
}

/// Build flat session rows by parsing and analyzing each session. Ports
/// `buildSessionRows`.
pub fn build_session_rows(sessions: &[SessionResult]) -> Result<Vec<SessionRow>> {
    let mut rows = Vec::with_capacity(sessions.len());
    for session in sessions {
        let entries = parse_transcript_file(&session.path);
        let analysis = analyze_session(&entries);
        let model = get_primary_model(
            analysis.opus_tokens,
            analysis.sonnet_tokens,
            analysis.haiku_tokens,
            analysis.fable_tokens,
        );
        let project = extract_project_name(&session.project);
        let cost = estimate_cost(&analysis);

        rows.push(SessionRow {
            session_path: session.path.to_string_lossy().to_string(),
            project,
            model: model.to_string(),
            input_tokens: analysis.input_tokens,
            output_tokens: analysis.output_tokens,
            total_tokens: analysis.input_tokens + analysis.output_tokens,
            cost,
            date: session.date.clone(),
        });
    }
    Ok(rows)
}

/// Format session rows as a pretty-printed JSON string (2-space indent, like
/// `JSON.stringify(rows, null, 2)`).
pub fn format_json(rows: &[SessionRow]) -> String {
    serde_json::to_string_pretty(rows).unwrap_or_else(|_| "[]".to_string())
}

/// Format a floating-point value the way JavaScript's `String(number)` would —
/// no trailing `.0` for integers (e.g. `0` not `0.0`, `0.0525` unchanged).
fn num_to_string(n: f64) -> String {
    format!("{n}")
}

/// Format session rows as a CSV string. Header and column order match
/// `formatCsv`.
pub fn format_csv(rows: &[SessionRow]) -> String {
    let header = "session_path,project,model,input_tokens,output_tokens,total_tokens,cost,date";
    let mut lines = vec![header.to_string()];
    for r in rows {
        lines.push(format!(
            "\"{}\",\"{}\",{},{},{},{},{},{}",
            r.session_path,
            r.project,
            r.model,
            r.input_tokens,
            r.output_tokens,
            r.total_tokens,
            num_to_string(r.cost),
            r.date,
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> SessionRow {
        SessionRow {
            session_path: "/home/user/.claude/projects/test/abc123.jsonl".to_string(),
            project: "my-project".to_string(),
            model: "Opus".to_string(),
            input_tokens: 1000,
            output_tokens: 500,
            total_tokens: 1500,
            cost: 0.0525,
            date: "2025-06-15".to_string(),
        }
    }

    // ── formatJson ──

    #[test]
    fn json_empty_array() {
        let parsed: serde_json::Value = serde_json::from_str(&format_json(&[])).unwrap();
        assert_eq!(parsed, serde_json::json!([]));
    }

    #[test]
    fn json_serializes_all_fields() {
        let parsed: serde_json::Value = serde_json::from_str(&format_json(&[row()])).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let r = &arr[0];
        assert_eq!(r["sessionPath"], "/home/user/.claude/projects/test/abc123.jsonl");
        assert_eq!(r["project"], "my-project");
        assert_eq!(r["model"], "Opus");
        assert_eq!(r["inputTokens"], 1000);
        assert_eq!(r["outputTokens"], 500);
        assert_eq!(r["totalTokens"], 1500);
        assert_eq!(r["cost"], 0.0525);
        assert_eq!(r["date"], "2025-06-15");
    }

    #[test]
    fn json_multiple_rows() {
        let rows = [
            SessionRow {
                session_path: "/a.jsonl".to_string(),
                project: "proj-a".to_string(),
                model: "Opus".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                total_tokens: 150,
                cost: 0.005,
                date: "2025-06-15".to_string(),
            },
            SessionRow {
                session_path: "/b.jsonl".to_string(),
                project: "proj-b".to_string(),
                model: "Sonnet".to_string(),
                input_tokens: 200,
                output_tokens: 100,
                total_tokens: 300,
                cost: 0.002,
                date: "2025-06-16".to_string(),
            },
        ];
        let parsed: serde_json::Value = serde_json::from_str(&format_json(&rows)).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["project"], "proj-a");
        assert_eq!(arr[1]["project"], "proj-b");
    }

    // ── formatCsv ──

    #[test]
    fn csv_header_only_for_empty() {
        assert_eq!(
            format_csv(&[]),
            "session_path,project,model,input_tokens,output_tokens,total_tokens,cost,date"
        );
    }

    #[test]
    fn csv_formats_rows_with_quoting() {
        let csv = format_csv(&[row()]);
        let lines: Vec<&str> = csv.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "session_path,project,model,input_tokens,output_tokens,total_tokens,cost,date"
        );
        assert!(lines[1].contains("\"my-project\""));
        assert!(lines[1].contains("1000"));
        assert!(lines[1].contains("500"));
        assert!(lines[1].contains("1500"));
        assert!(lines[1].contains("0.0525"));
        assert!(lines[1].contains("2025-06-15"));
    }

    #[test]
    fn csv_multiple_rows() {
        let rows = [
            SessionRow {
                session_path: "/a.jsonl".to_string(),
                project: "proj-a".to_string(),
                model: "Opus".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                total_tokens: 150,
                cost: 0.005,
                date: "2025-06-15".to_string(),
            },
            SessionRow {
                session_path: "/b.jsonl".to_string(),
                project: "proj-b".to_string(),
                model: "Sonnet".to_string(),
                input_tokens: 200,
                output_tokens: 100,
                total_tokens: 300,
                cost: 0.002,
                date: "2025-06-16".to_string(),
            },
        ];
        assert_eq!(format_csv(&rows).split('\n').count(), 3);
    }

    #[test]
    fn cost_formatting_drops_trailing_zero() {
        assert_eq!(num_to_string(0.0), "0");
        assert_eq!(num_to_string(0.0525), "0.0525");
        assert_eq!(num_to_string(0.005), "0.005");
    }

    // ── estimateCost ──

    fn analysis(input: i64, output: i64, haiku: i64, fable: i64) -> SessionAnalysis {
        SessionAnalysis {
            input_tokens: input,
            output_tokens: output,
            opus_tokens: 0,
            sonnet_tokens: 0,
            haiku_tokens: haiku,
            fable_tokens: fable,
            cache_hit_rate: 0.0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            repeated_reads: 0,
            model_efficiency: 0.0,
        }
    }

    #[test]
    fn cost_dilutes_across_unpriced_fable_tokens() {
        // 1M fable + 10K haiku. Only ~1% of the tokens belong to (priced) haiku,
        // so the estimate is ~1% of the naive "all haiku" figure — Fable dilutes
        // the fraction but is itself priced at 0.
        let a = analysis(1_000_000, 0, 10_000, 1_000_000);
        let cost = estimate_cost(&a);

        let naive_all_haiku = 1_000_000.0 * 0.8 / 1_000_000.0; // 0.8
        let expected = naive_all_haiku * (10_000.0 / 1_010_000.0); // ~0.00792
        assert!((cost - expected).abs() < 0.001, "cost={cost} expected≈{expected}");
        assert!(cost < naive_all_haiku * 0.02, "cost {cost} should be ~1% of {naive_all_haiku}");
    }

    #[test]
    fn cost_all_fable_is_zero() {
        // Fable has no price in the table, so an all-Fable session costs nothing
        // (and, crucially, does not panic on the missing price).
        let a = analysis(1_000_000, 500_000, 0, 1_000_000);
        assert_eq!(estimate_cost(&a), 0.0);
    }
}
