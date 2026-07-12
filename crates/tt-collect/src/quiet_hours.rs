//! Working-hours gate for the token-costing calendar collector.
//!
//! The calendar collector is the only collector that spends `claude` tokens per
//! tick, yet when enabled it would otherwise run around the clock — including
//! nights and weekends when there's no meeting to count down to. This module
//! decides, from an injected `now_ms`, whether the current local time falls
//! inside the configured working-hours window (see [`tt_config::CalendarQuietHours`]).
//!
//! The clock is never read here: [`should_run_calendar`] converts the caller's
//! `now_ms` to local time at the boundary and hands the plain
//! `(weekday, hour, minute)` to the pure [`should_run_at`], which holds all the
//! window logic and is exhaustively unit-tested.

use chrono::{Datelike, Local, TimeZone, Timelike, Weekday};
use tt_config::CalendarQuietHours;

/// Whether the calendar collector may run at the instant `now_ms` (epoch ms),
/// given the quiet-hours config. Converts `now_ms` to the system-local time
/// zone and defers to [`should_run_at`].
///
/// On an ambiguous local instant (a fall-back DST overlap) either candidate is
/// close enough for an hour-granularity window, so the earlier one is used. On a
/// nonexistent instant (a spring-forward gap — `now_ms` can't actually name such
/// a wall-clock time, but the conversion is total) the gate errs open and lets
/// the run proceed rather than silently suppressing it.
pub fn should_run_calendar(now_ms: i64, quiet: &CalendarQuietHours) -> bool {
    if !quiet.enabled {
        return true;
    }
    let local = match Local.timestamp_millis_opt(now_ms) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => dt,
        chrono::LocalResult::None => return true,
    };
    should_run_at(local.weekday(), local.hour(), local.minute(), quiet)
}

/// Pure window test: is `(weekday, hour, minute)` inside the quiet-hours window?
///
/// The collector runs when the day is in `quiet.weekdays` (0 = Monday … 6 =
/// Sunday) *and* the minute-of-day is in `[start_hour:00, end_hour:00)` — the
/// start minute is included, the end hour's `:00` is excluded. A disabled config
/// always returns `true`.
pub fn should_run_at(weekday: Weekday, hour: u32, minute: u32, quiet: &CalendarQuietHours) -> bool {
    if !quiet.enabled {
        return true;
    }
    let dow = weekday.num_days_from_monday() as u8;
    if !quiet.weekdays.contains(&dow) {
        return false;
    }
    let mins = hour * 60 + minute;
    let start = quiet.start_hour as u32 * 60;
    let end = quiet.end_hour as u32 * 60;
    mins >= start && mins < end
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default config: 8:00–18:00 local, Mon–Fri, enabled.
    fn default_quiet() -> CalendarQuietHours {
        CalendarQuietHours::default()
    }

    #[test]
    fn window_and_weekday_matrix() {
        let q = default_quiet();
        // (weekday, hour, minute, expected)
        let cases: &[(Weekday, u32, u32, bool)] = &[
            // Weekday, inside the window.
            (Weekday::Mon, 8, 0, true),   // start edge, :00 included
            (Weekday::Mon, 8, 1, true),   // one minute past start
            (Weekday::Wed, 12, 30, true), // mid-window
            (Weekday::Fri, 17, 59, true), // last runnable minute
            // Weekday, window edges that must NOT run.
            (Weekday::Mon, 7, 59, false), // one minute before start
            (Weekday::Mon, 18, 0, false), // end edge, :00 excluded
            (Weekday::Mon, 18, 1, false), // past the end
            (Weekday::Tue, 0, 0, false),  // midnight
            (Weekday::Thu, 23, 59, false),
            // Weekend, even inside working hours → never runs.
            (Weekday::Sat, 12, 0, false),
            (Weekday::Sun, 9, 30, false),
            (Weekday::Sun, 8, 0, false),
        ];
        for &(wd, h, m, expected) in cases {
            assert_eq!(
                should_run_at(wd, h, m, &q),
                expected,
                "weekday={wd:?} time={h:02}:{m:02} expected run={expected}"
            );
        }
    }

    #[test]
    fn disabled_config_always_runs() {
        let q = CalendarQuietHours { enabled: false, ..CalendarQuietHours::default() };
        // Every (weekday, hour) — including the dead of a weekend night — runs.
        for wd in [Weekday::Mon, Weekday::Sat, Weekday::Sun] {
            for h in [0, 3, 12, 18, 23] {
                assert!(should_run_at(wd, h, 0, &q), "disabled must always run: {wd:?} {h}:00");
            }
        }
    }

    #[test]
    fn custom_window_and_weekend_mask() {
        // A night-owl config: runs 22:00–02:00 would wrap midnight, which this
        // simple window doesn't support; instead prove a non-default same-day
        // window plus a weekend-only mask (Sat+Sun) works.
        let q = CalendarQuietHours {
            enabled: true,
            start_hour: 10,
            end_hour: 14,
            weekdays: vec![5, 6], // Sat, Sun
        };
        assert!(should_run_at(Weekday::Sat, 10, 0, &q), "start edge on a listed day");
        assert!(should_run_at(Weekday::Sun, 13, 59, &q), "last minute on a listed day");
        assert!(!should_run_at(Weekday::Sat, 14, 0, &q), "end edge excluded");
        assert!(!should_run_at(Weekday::Sat, 9, 59, &q), "before start");
        assert!(!should_run_at(Weekday::Mon, 11, 0, &q), "weekday not in the mask");
        assert!(!should_run_at(Weekday::Fri, 11, 0, &q), "weekday not in the mask");
    }

    #[test]
    fn empty_weekday_mask_never_runs() {
        let q = CalendarQuietHours { weekdays: vec![], ..CalendarQuietHours::default() };
        for wd in [Weekday::Mon, Weekday::Wed, Weekday::Sun] {
            assert!(!should_run_at(wd, 12, 0, &q), "no listed weekday means never: {wd:?}");
        }
    }

    #[test]
    fn should_run_calendar_uses_local_now() {
        // Build a local instant inside the default window (Wed 2026-07-15 12:00
        // local) and one outside it (Sat 2026-07-18 12:00 local).
        let inside = Local.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap().timestamp_millis();
        let weekend = Local.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap().timestamp_millis();
        let night = Local.with_ymd_and_hms(2026, 7, 15, 3, 0, 0).unwrap().timestamp_millis();
        let q = default_quiet();
        assert!(should_run_calendar(inside, &q), "Wed noon is inside the window");
        assert!(!should_run_calendar(weekend, &q), "Saturday is masked out");
        assert!(!should_run_calendar(night, &q), "3am is outside the window");
        // Disabled → runs regardless of when.
        let off = CalendarQuietHours { enabled: false, ..q };
        assert!(should_run_calendar(weekend, &off));
        assert!(should_run_calendar(night, &off));
    }
}
