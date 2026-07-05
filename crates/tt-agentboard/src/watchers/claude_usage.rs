//! Usage-token extraction for the claude-code watcher. Ports slot-1
//! `runtime/agents/watchers/claude-usage.ts`.

use tt_claude_code::{TranscriptEntry, Usage};

const FIVE_MIN_MS: i64 = 5 * 60 * 1000;
const ONE_HOUR_MS: i64 = 60 * 60 * 1000;

/// Token/model/cache summary for one session. Ports `ClaudeUsageSummary`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeUsageSummary {
    pub model: String,
    pub context_used: i64,
    pub context_max: i64,
    pub cache_ttl_ms: Option<i64>,
    pub cache_expires_at: Option<i64>,
    pub last_activity_at: i64,
}

/// Context window size for a model: 1M when the name ends in `[1m]`, else 200K.
/// Ports `contextMax`.
pub fn context_max(model: &str) -> i64 {
    if model.to_ascii_lowercase().ends_with("[1m]") { 1_000_000 } else { 200_000 }
}

/// Total context tokens = input + output + cache_read + cache_creation. Ports `contextUsed`.
pub fn context_used(u: &Usage) -> i64 {
    u.input_tokens.unwrap_or(0)
        + u.output_tokens.unwrap_or(0)
        + u.cache_read_input_tokens.unwrap_or(0)
        + u.cache_creation_input_tokens.unwrap_or(0)
}

/// Cache TTL: 1h when any 1h ephemeral tokens, else 5m when 5m ephemeral or any
/// cache reads, else none. Ports `cacheTtlMs`.
pub fn cache_ttl_ms(u: &Usage) -> Option<i64> {
    let h = u.cache_creation.as_ref().and_then(|c| c.ephemeral_1h_input_tokens).unwrap_or(0);
    let m = u.cache_creation.as_ref().and_then(|c| c.ephemeral_5m_input_tokens).unwrap_or(0);
    let reads = u.cache_read_input_tokens.unwrap_or(0);
    if h > 0 {
        Some(ONE_HOUR_MS)
    } else if m > 0 || reads > 0 {
        Some(FIVE_MIN_MS)
    } else {
        None
    }
}

/// The usage summary from the newest assistant entry that has a `usage` block and
/// a parseable timestamp. Ports `extractUsageSummary`.
pub fn extract_usage_summary(entries: &[TranscriptEntry]) -> Option<ClaudeUsageSummary> {
    for entry in entries.iter().rev() {
        let Some(msg) = &entry.message else {
            continue;
        };
        if msg.role.as_deref() != Some("assistant") {
            continue;
        }
        let Some(usage) = &msg.usage else {
            continue;
        };
        let Some(ts) = entry.timestamp.as_deref().and_then(super::claude_code::parse_timestamp_ms)
        else {
            continue;
        };

        let model = msg.model.clone().unwrap_or_default();
        let ttl = cache_ttl_ms(usage);
        return Some(ClaudeUsageSummary {
            context_used: context_used(usage),
            context_max: context_max(&model),
            cache_ttl_ms: ttl,
            cache_expires_at: ttl.map(|t| ts + t),
            last_activity_at: ts,
            model,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(json: serde_json::Value) -> Usage {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn context_max_detects_1m_suffix() {
        assert_eq!(context_max("claude-sonnet-4-5[1m]"), 1_000_000);
        assert_eq!(context_max("claude-opus-4-6"), 200_000);
        assert_eq!(context_max("claude-haiku-4-5"), 200_000);
        assert_eq!(context_max(""), 200_000);
        assert_eq!(context_max("gpt-4"), 200_000);
    }

    #[test]
    fn context_used_sums_all_token_buckets() {
        assert_eq!(
            context_used(&usage(serde_json::json!({
                "input_tokens": 100, "output_tokens": 50,
                "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 200
            }))),
            1350
        );
        assert_eq!(context_used(&usage(serde_json::json!({ "input_tokens": 10 }))), 10);
        assert_eq!(context_used(&usage(serde_json::json!({}))), 0);
    }

    #[test]
    fn cache_ttl_rules() {
        assert_eq!(cache_ttl_ms(&usage(serde_json::json!({ "input_tokens": 100 }))), None);
        assert_eq!(
            cache_ttl_ms(&usage(serde_json::json!({
                "cache_creation": { "ephemeral_1h_input_tokens": 100, "ephemeral_5m_input_tokens": 0 }
            }))),
            Some(ONE_HOUR_MS)
        );
        assert_eq!(
            cache_ttl_ms(&usage(serde_json::json!({
                "cache_creation": { "ephemeral_1h_input_tokens": 0, "ephemeral_5m_input_tokens": 100 }
            }))),
            Some(FIVE_MIN_MS)
        );
        assert_eq!(
            cache_ttl_ms(&usage(serde_json::json!({ "cache_read_input_tokens": 500 }))),
            Some(FIVE_MIN_MS)
        );
        // Prefers 1h when both present.
        assert_eq!(
            cache_ttl_ms(&usage(serde_json::json!({
                "cache_creation": { "ephemeral_1h_input_tokens": 50, "ephemeral_5m_input_tokens": 100 }
            }))),
            Some(ONE_HOUR_MS)
        );
    }

    fn assistant(ts: &str, model: &str, usage: serde_json::Value) -> TranscriptEntry {
        serde_json::from_value(serde_json::json!({
            "type": "assistant",
            "timestamp": ts,
            "message": { "role": "assistant", "model": model, "usage": usage }
        }))
        .unwrap()
    }

    #[test]
    fn extract_returns_none_for_empty_or_no_usage() {
        assert!(extract_usage_summary(&[]).is_none());
        let user: TranscriptEntry = serde_json::from_value(serde_json::json!({
            "type": "user", "timestamp": "2026-04-12T00:00:00Z",
            "message": { "role": "user", "content": "hi" }
        }))
        .unwrap();
        assert!(extract_usage_summary(&[user]).is_none());
    }

    #[test]
    fn extract_uses_newest_assistant_with_usage_and_1h_cache() {
        let entries = vec![
            assistant(
                "2026-04-12T00:00:00Z",
                "claude-opus-4-6",
                serde_json::json!({ "input_tokens": 10, "output_tokens": 5 }),
            ),
            assistant(
                "2026-04-12T00:05:00Z",
                "claude-opus-4-6",
                serde_json::json!({
                    "input_tokens": 1, "output_tokens": 249,
                    "cache_read_input_tokens": 50612, "cache_creation_input_tokens": 2297,
                    "cache_creation": { "ephemeral_1h_input_tokens": 2297, "ephemeral_5m_input_tokens": 0 }
                }),
            ),
        ];
        let r = extract_usage_summary(&entries).unwrap();
        assert_eq!(r.model, "claude-opus-4-6");
        assert_eq!(r.context_used, 53159);
        assert_eq!(r.context_max, 200_000);
        assert_eq!(r.cache_ttl_ms, Some(ONE_HOUR_MS));
        let expected_ts =
            super::super::claude_code::parse_timestamp_ms("2026-04-12T00:05:00Z").unwrap();
        assert_eq!(r.last_activity_at, expected_ts);
        assert_eq!(r.cache_expires_at, Some(expected_ts + ONE_HOUR_MS));
    }

    #[test]
    fn extract_leaves_cache_null_when_no_cache_activity() {
        let entries = vec![assistant(
            "2026-04-12T00:00:00Z",
            "claude-opus-4-6",
            serde_json::json!({ "input_tokens": 100, "output_tokens": 50 }),
        )];
        let r = extract_usage_summary(&entries).unwrap();
        assert_eq!(r.cache_ttl_ms, None);
        assert_eq!(r.cache_expires_at, None);
    }

    #[test]
    fn context_max_from_1m_model_in_summary() {
        let entries = vec![assistant(
            "2026-04-12T00:00:00Z",
            "claude-sonnet-4-5[1m]",
            serde_json::json!({ "input_tokens": 5 }),
        )];
        let r = extract_usage_summary(&entries).unwrap();
        assert_eq!(r.context_max, 1_000_000);
    }
}
