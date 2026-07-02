//! Path-template rendering, ported from `src/commands/journal/paths.ts` and the
//! Monday-of-week helper from `src/lib/date-utils.ts`.
//!
//! The TS CLI resolves path templates with Luxon (`DateTime.toFormat`). Because both
//! CLIs share the same settings file, the rendered paths must match byte-for-byte. We
//! therefore map the Luxon tokens **actually used by the default templates** to chrono
//! by hand rather than pulling in a full Luxon-compatible formatter.
//!
//! Supported Luxon tokens: `yyyy` (4-digit year), `MM` (2-digit month), `dd` (2-digit
//! day). These appear both bare (`{yyyy}`) and as `monday:`-prefixed compound formats
//! (`{monday:yyyy-MM-dd}`). Any other character in a token is emitted literally — we do
//! **not** implement the rest of the Luxon token vocabulary (see MIGRATION.md).

use crate::JournalType;
use chrono::{Datelike, Duration, NaiveDate};
use std::path::{Path, PathBuf};
use tt_config::JournalSettings;

/// Result of resolving a journal file path for a given date and type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFileInfo {
    /// Absolute path (base folder joined with the resolved template).
    pub full_path: PathBuf,
    /// The Monday used to render `monday:` tokens (for daily notes) or the date itself.
    pub monday_date: NaiveDate,
    /// The date the path was resolved against.
    pub current_date: NaiveDate,
}

/// Monday of the week containing `date`. Ports `getMondayOfWeek` in `date-utils.ts`:
/// Sunday is treated as the *last* day of the Monday-started week (so it maps back to
/// the previous Monday), matching ISO-week semantics.
pub fn monday_of_week(date: NaiveDate) -> NaiveDate {
    let back = date.weekday().num_days_from_monday() as i64;
    date - Duration::days(back)
}

/// Slugify a title like the TS does: lowercase, then collapse each run of whitespace to
/// a single `-` (`title.toLowerCase().replace(/\s+/g, "-")`). Leading/trailing
/// whitespace runs also become `-`, matching JS `replace` semantics.
pub fn slugify(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut out = String::new();
    let mut in_ws = false;
    for ch in lower.chars() {
        if ch.is_whitespace() {
            in_ws = true;
        } else {
            if in_ws {
                out.push('-');
                in_ws = false;
            }
            out.push(ch);
        }
    }
    if in_ws {
        out.push('-');
    }
    out
}

/// Render a Luxon-style format string (the supported subset) against `date`.
fn render_luxon(fmt: &str, date: NaiveDate) -> String {
    let mut out = String::new();
    let mut rest = fmt;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("yyyy") {
            out.push_str(&format!("{:04}", date.year()));
            rest = r;
        } else if let Some(r) = rest.strip_prefix("MM") {
            out.push_str(&format!("{:02}", date.month()));
            rest = r;
        } else if let Some(r) = rest.strip_prefix("dd") {
            out.push_str(&format!("{:02}", date.day()));
            rest = r;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

/// Replace `{token}` occurrences in `template`. Ports `resolvePathTemplate`:
/// `{title}` -> slugified title, `{monday:FMT}` -> `FMT` rendered against `monday_date`,
/// and any other `{FMT}` -> `FMT` rendered against `date`. Text outside braces, and any
/// unterminated `{`, is emitted verbatim.
pub fn resolve_path_template(
    template: &str,
    title: &str,
    date: NaiveDate,
    monday_date: NaiveDate,
) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let token = &after[..close];
                out.push_str(&render_token(token, title, date, monday_date));
                rest = &after[close + 1..];
            }
            None => {
                // Unterminated brace: emit the rest verbatim (matches the JS regex,
                // which only matches a complete `{...}`).
                out.push('{');
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn render_token(token: &str, title: &str, date: NaiveDate, monday_date: NaiveDate) -> String {
    if token == "title" {
        return slugify(title);
    }
    if let Some(monday_fmt) = token.strip_prefix("monday:") {
        return render_luxon(monday_fmt, monday_date);
    }
    render_luxon(token, date)
}

/// Resolve the full journal file path for a type + date + title. Ports
/// `generateJournalFileInfoByType`: daily notes render `monday:` tokens against the
/// week's Monday; meetings and notes render everything against the date itself.
pub fn generate_journal_file_info(
    settings: &JournalSettings,
    date: NaiveDate,
    ty: JournalType,
    title: &str,
) -> JournalFileInfo {
    let (template, monday_date) = match ty {
        JournalType::DailyNotes => (&settings.daily_path_template, monday_of_week(date)),
        JournalType::Meeting => (&settings.meeting_path_template, date),
        JournalType::Note => (&settings.note_path_template, date),
    };

    let resolved = resolve_path_template(template, title, date, monday_date);
    let full_path = Path::new(&settings.base_folder).join(resolved);

    JournalFileInfo { full_path, monday_date, current_date: date }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn monday_of_week_from_each_weekday() {
        // Week of Mon 2026-06-29 .. Sun 2026-07-05.
        let monday = ymd(2026, 6, 29);
        assert_eq!(monday_of_week(ymd(2026, 6, 29)), monday); // Monday
        assert_eq!(monday_of_week(ymd(2026, 7, 1)), monday); // Wednesday
        assert_eq!(monday_of_week(ymd(2026, 7, 3)), monday); // Friday
        assert_eq!(monday_of_week(ymd(2026, 7, 5)), monday); // Sunday -> previous Monday
    }

    #[test]
    fn monday_of_week_across_year_boundary() {
        // 2026-01-01 is a Thursday; its Monday is in the previous year.
        assert_eq!(monday_of_week(ymd(2026, 1, 1)), ymd(2025, 12, 29));
    }

    #[test]
    fn slugify_matches_ts_semantics() {
        assert_eq!(slugify("Team Sync"), "team-sync");
        assert_eq!(slugify("Weekly   Standup"), "weekly-standup");
        assert_eq!(slugify("  Padded  "), "-padded-");
        // Non-space characters are preserved (TS only lowercases + collapses whitespace).
        assert_eq!(slugify("Q3 Plan!"), "q3-plan!");
    }

    #[test]
    fn daily_path_golden() {
        // Default daily template rendered against a Wednesday resolves to that week's
        // Monday (2026-06-29). Matches the TS/Luxon output byte-for-byte.
        let settings = JournalSettings { base_folder: "/home/u".to_string(), ..Default::default() };
        let info =
            generate_journal_file_info(&settings, ymd(2026, 7, 1), JournalType::DailyNotes, "");
        assert_eq!(
            info.full_path,
            PathBuf::from("/home/u/journal/2026/06/daily-notes/2026-06-29-daily-notes.md"),
        );
        assert_eq!(info.monday_date, ymd(2026, 6, 29));
    }

    #[test]
    fn meeting_and_note_path_golden() {
        let settings = JournalSettings { base_folder: "/base".to_string(), ..Default::default() };
        let meeting = generate_journal_file_info(
            &settings,
            ymd(2026, 7, 1),
            JournalType::Meeting,
            "Team Sync",
        );
        assert_eq!(
            meeting.full_path,
            PathBuf::from("/base/journal/2026/07/meetings/2026-07-01-team-sync.md"),
        );

        let note =
            generate_journal_file_info(&settings, ymd(2026, 7, 1), JournalType::Note, "My Idea");
        assert_eq!(
            note.full_path,
            PathBuf::from("/base/journal/2026/07/notes/2026-07-01-my-idea.md"),
        );
    }

    #[test]
    fn resolve_handles_compound_and_unknown_tokens() {
        let d = ymd(2026, 7, 1);
        // Compound Luxon format (as used in the daily template header).
        assert_eq!(
            resolve_path_template("{monday:yyyy-MM-dd}", "", d, ymd(2026, 6, 29)),
            "2026-06-29"
        );
        // Literal text around tokens is preserved.
        assert_eq!(resolve_path_template("x/{yyyy}/y", "", d, d), "x/2026/y");
        // Unterminated brace is emitted verbatim.
        assert_eq!(resolve_path_template("a{yyyy", "", d, d), "a{yyyy");
    }
}
