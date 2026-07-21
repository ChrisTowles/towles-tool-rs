//! Small text helpers. Ports §task§-1 `runtime/text-utils.ts`.

/// Truncate a string to `max` characters, appending an ellipsis if clipped.
///
/// Ports the TS `truncate(s, max)`. The TS uses UTF-16 units (`String.length` /
/// `slice`); this counts Unicode scalar values instead, which matches for BMP
/// text and never splits a UTF-8 boundary. (Deviation noted in the port report.)
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_through_short_strings() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn clips_and_appends_ellipsis() {
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("abcdef", 3), "ab…");
    }
}
