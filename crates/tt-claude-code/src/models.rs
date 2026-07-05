//! Model → context-window lookup.
//!
//! **This is a hand-maintained table — KEEP IT UPDATED as models ship.** Context
//! windows are a property of the model *and the platform it runs on*, and both
//! move over time, so there's no way to derive this offline; the transcript only
//! gives us the model id string. Anything not matched falls back to the
//! conservative 200K default, and the caller's observed-usage floor (see
//! `tt-agentboard`'s `context_max`) still corrects an under-estimate the moment a
//! session's prompt exceeds the table's guess.
//!
//! **Last reviewed: 2026-07-05** (source: Anthropic model docs).
//!
//! Current tiers:
//! - **1M-native** (first-party API, Vertex AI, Claude Platform on AWS): Fable 5,
//!   Mythos 5, Opus 4.6 / 4.7 / 4.8, Sonnet 4.6 / 5. Listed in [`ONE_M_MODELS`].
//! - **200K**: Haiku (all), Opus 4.5 and earlier, Sonnet 4.5 and earlier — and
//!   the fallback for anything unrecognized.
//! - **Amazon Bedrock** (`anthropic.<id>` prefix): treated as **200K** for now,
//!   regardless of model. Revisit if/when Bedrock serves 1M for these models.
//! - An explicit `[1m]` marker on the model string (the older Sonnet 4/4.5 1M
//!   opt-in) always forces 1M.

/// The base 200K context window.
pub const CONTEXT_200K: i64 = 200_000;
/// The extended 1M context window.
pub const CONTEXT_1M: i64 = 1_000_000;

/// Model-id substrings whose family serves a **1M** context window on the
/// first-party API / Vertex / Claude Platform on AWS. Match is case-insensitive
/// `contains`, so `claude-opus-4-8`, `claude-opus-4-8-foo`, etc. all hit. KEEP
/// UPDATED — add new 1M families here as they ship.
pub const ONE_M_MODELS: &[&str] = &[
    "fable-5",
    "mythos", // mythos-5 and mythos-preview
    "opus-4-6",
    "opus-4-7",
    "opus-4-8",
    "sonnet-4-6",
    "sonnet-5",
];

/// Context-window size (in tokens) for a model id, per the maintained table above.
///
/// Precedence: an explicit `[1m]` marker → 1M; a Bedrock id (`anthropic.` prefix)
/// → 200K; a known 1M family → 1M; otherwise the conservative 200K default.
pub fn context_window(model: &str) -> i64 {
    let m = model.to_ascii_lowercase();

    // Explicit 1M opt-in marker (Sonnet 4/4.5 era) always wins.
    if m.ends_with("[1m]") {
        return CONTEXT_1M;
    }
    // Amazon Bedrock currently serves a 200K window for these models. KEEP UPDATED.
    if m.starts_with("anthropic.") {
        return CONTEXT_200K;
    }
    if ONE_M_MODELS.iter().any(|needle| m.contains(needle)) {
        return CONTEXT_1M;
    }
    // Haiku, Opus/Sonnet 4.5 and earlier, and anything unrecognized.
    CONTEXT_200K
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_m_native_families() {
        assert_eq!(context_window("claude-opus-4-8"), CONTEXT_1M);
        assert_eq!(context_window("claude-opus-4-7"), CONTEXT_1M);
        assert_eq!(context_window("claude-opus-4-6"), CONTEXT_1M);
        assert_eq!(context_window("claude-sonnet-5"), CONTEXT_1M);
        assert_eq!(context_window("claude-sonnet-4-6"), CONTEXT_1M);
        assert_eq!(context_window("claude-fable-5"), CONTEXT_1M);
        assert_eq!(context_window("claude-mythos-5"), CONTEXT_1M);
    }

    #[test]
    fn two_hundred_k_families_and_unknown() {
        assert_eq!(context_window("claude-haiku-4-5"), CONTEXT_200K);
        assert_eq!(context_window("claude-opus-4-5"), CONTEXT_200K); // legacy Opus
        assert_eq!(context_window("claude-sonnet-4-5"), CONTEXT_200K); // 1M was beta-only
        assert_eq!(context_window("claude-opus-4-9-future"), CONTEXT_200K); // unrecognized → conservative
        assert_eq!(context_window(""), CONTEXT_200K);
    }

    #[test]
    fn one_m_marker_forces_1m() {
        // The old Sonnet 4/4.5 opt-in — a 200K-native family with the [1m] marker.
        assert_eq!(context_window("claude-sonnet-4-5[1m]"), CONTEXT_1M);
    }

    #[test]
    fn bedrock_prefix_caps_at_200k() {
        // Same model, Bedrock prefix → 200K even though first-party is 1M.
        assert_eq!(context_window("anthropic.claude-opus-4-8"), CONTEXT_200K);
        assert_eq!(context_window("claude-opus-4-8"), CONTEXT_1M);
    }

    #[test]
    fn distinguishes_sonnet_5_from_sonnet_4_5() {
        assert_eq!(context_window("claude-sonnet-5"), CONTEXT_1M);
        assert_eq!(context_window("claude-sonnet-4-5"), CONTEXT_200K);
    }
}
