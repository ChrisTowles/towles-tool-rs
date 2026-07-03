//! Small text helpers. Ports slot-1 `runtime/text-utils.ts`.

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

/// Human session uptime from an epoch-seconds creation time. Ports the TS
/// `formatUptime` (`3d2h` / `5h42m` / `12m`; empty for bad input).
pub fn format_uptime(created_at_s: i64, now_s: i64) -> String {
    let diff = now_s - created_at_s;
    if diff < 0 {
        return String::new();
    }
    let days = diff / 86_400;
    let hours = (diff % 86_400) / 3_600;
    let mins = (diff % 3_600) / 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else {
        format!("{mins}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_formats_by_magnitude() {
        assert_eq!(format_uptime(0, 12 * 60), "12m");
        assert_eq!(format_uptime(0, 5 * 3600 + 42 * 60), "5h42m");
        assert_eq!(format_uptime(0, 3 * 86_400 + 2 * 3_600), "3d2h");
        assert_eq!(format_uptime(100, 50), "");
    }

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
