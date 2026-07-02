//! Template scaffolding, content creation, listing, and search.
//!
//! Ports `templates.ts`, `list.ts`, and `search.ts`. The default template strings are
//! copied verbatim from `getDefault*Template` in `templates.ts`. As in the TS CLI, an
//! external template file in `template_dir` (written on first run) takes precedence over
//! the built-in fallback string.

use crate::tokens::monday_of_week;
use crate::{Error, JournalType, Result};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// Default template file names (mirrors `TEMPLATE_FILES` in templates.ts).
const TEMPLATE_FILE_DAILY: &str = "daily-notes.md";
const TEMPLATE_FILE_MEETING: &str = "meeting.md";
const TEMPLATE_FILE_NOTE: &str = "note.md";

// Built-in template strings, copied verbatim from `getDefault*Template` in templates.ts.
const DEFAULT_DAILY_TEMPLATE: &str = "# Journal for Week {monday:yyyy-MM-dd}

## {monday:yyyy-MM-dd} Monday

## {tuesday:yyyy-MM-dd} Tuesday

## {wednesday:yyyy-MM-dd} Wednesday

## {thursday:yyyy-MM-dd} Thursday

## {friday:yyyy-MM-dd} Friday
";

const DEFAULT_MEETING_TEMPLATE: &str = "# Meeting: {title}

**Date:** {date}
**Time:** {time}
**Attendees:**

## Agenda

-

## Notes

## Action Items

- [ ]

## Follow-up
";

const DEFAULT_NOTE_TEMPLATE: &str = "# {title}

**Created:** {date} {time}

## Summary

## Details

## References
";

// ---------------------------------------------------------------------------
// Date/time formatting helpers (match `date-utils.ts` and `templates.ts`).
// ---------------------------------------------------------------------------

/// Format a date as `YYYY-MM-DD` (matches `formatDate`, i.e. `toLocaleDateString("en-CA")`).
pub fn format_date(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// Format a time as 24-hour `HH:mm` (matches `formatTime`).
fn format_time(dt: NaiveDateTime) -> String {
    format!("{:02}:{:02}", dt.hour(), dt.minute())
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// Replace `{key}` occurrences using `vars`; unknown keys are left verbatim.
/// Ports the `renderTemplate` helper in templates.ts (a plain map lookup — no Luxon).
fn render_template(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                match vars.get(key) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Load an external template file, returning `None` if it does not exist.
fn load_template(template_dir: &Path, file: &str) -> Option<String> {
    let path = template_dir.join(file);
    std::fs::read_to_string(path).ok()
}

/// Write the three default template files into `template_dir` if they are missing.
/// Ports `ensureTemplatesExist`.
pub fn ensure_templates_exist(template_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(template_dir)?;
    for (file, content) in [
        (TEMPLATE_FILE_DAILY, DEFAULT_DAILY_TEMPLATE),
        (TEMPLATE_FILE_MEETING, DEFAULT_MEETING_TEMPLATE),
        (TEMPLATE_FILE_NOTE, DEFAULT_NOTE_TEMPLATE),
    ] {
        let path = template_dir.join(file);
        if !path.exists() {
            std::fs::write(path, content)?;
        }
    }
    Ok(())
}

/// Build daily-notes content for the week starting at `monday_date`. Ports
/// `createJournalContent`: prefers the external `daily-notes.md` template, else the
/// built-in string.
pub fn create_journal_content(monday_date: NaiveDate, template_dir: Option<&Path>) -> String {
    let vars = weekday_vars(monday_date);
    if let Some(dir) = template_dir
        && let Some(external) = load_template(dir, TEMPLATE_FILE_DAILY)
    {
        return render_template(&external, &vars);
    }
    render_template(DEFAULT_DAILY_TEMPLATE, &vars)
}

fn weekday_vars(monday_date: NaiveDate) -> HashMap<&'static str, String> {
    let mut vars = HashMap::new();
    vars.insert("monday:yyyy-MM-dd", format_date(monday_date));
    vars.insert("tuesday:yyyy-MM-dd", format_date(monday_date + Duration::days(1)));
    vars.insert("wednesday:yyyy-MM-dd", format_date(monday_date + Duration::days(2)));
    vars.insert("thursday:yyyy-MM-dd", format_date(monday_date + Duration::days(3)));
    vars.insert("friday:yyyy-MM-dd", format_date(monday_date + Duration::days(4)));
    vars
}

/// Build meeting content. Ports `createMeetingContent`.
///
/// NOTE: the TS built-in fallback string carries trailing spaces on a few lines
/// (`**Attendees:** `, `- `, `- [ ] `) that the on-disk default template does not. Since
/// `ensure_templates_exist` always writes the (trailing-space-free) template first, the
/// fallback is normally unreachable — but we preserve the quirk for a faithful port.
pub fn create_meeting_content(
    title: &str,
    dt: NaiveDateTime,
    template_dir: Option<&Path>,
) -> String {
    let date_str = format_date(dt.date());
    let time_str = format_time(dt);
    let meeting_title = if title.is_empty() { "Meeting" } else { title };

    if let Some(dir) = template_dir
        && let Some(external) = load_template(dir, TEMPLATE_FILE_MEETING)
    {
        let mut vars = HashMap::new();
        vars.insert("title", meeting_title.to_string());
        vars.insert("date", date_str);
        vars.insert("time", time_str);
        return render_template(&external, &vars);
    }

    format!(
        "# Meeting: {meeting_title}\n\n**Date:** {date_str}\n**Time:** {time_str}\n**Attendees:** \n\n## Agenda\n\n- \n\n## Notes\n\n## Action Items\n\n- [ ] \n\n## Follow-up\n"
    )
}

/// Build note content. Ports `createNoteContent`.
pub fn create_note_content(title: &str, dt: NaiveDateTime, template_dir: Option<&Path>) -> String {
    let date_str = format_date(dt.date());
    let time_str = format_time(dt);
    let note_title = if title.is_empty() { "Note" } else { title };

    if let Some(dir) = template_dir
        && let Some(external) = load_template(dir, TEMPLATE_FILE_NOTE)
    {
        let mut vars = HashMap::new();
        vars.insert("title", note_title.to_string());
        vars.insert("date", date_str);
        vars.insert("time", time_str);
        return render_template(&external, &vars);
    }

    format!(
        "# {note_title}\n\n**Created:** {date_str} {time_str}\n\n## Summary\n\n## Details\n\n## References\n"
    )
}

// ---------------------------------------------------------------------------
// Listing and classification
// ---------------------------------------------------------------------------

/// Recursively collect all `.md` files under `dir`. Ports `collectMarkdownFiles`.
/// Directory entries are sorted for deterministic output (the TS relies on OS order).
pub fn collect_markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => return results,
    };
    entries.sort();
    for path in entries {
        if path.is_dir() {
            results.extend(collect_markdown_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            results.push(path);
        }
    }
    results
}

/// Infer a journal type from directory names in a path. Ports `inferTypeFromPath`.
pub fn infer_type_from_path(path: &Path) -> Option<JournalType> {
    let lower = path.to_string_lossy().to_lowercase();
    if lower.contains("daily-notes") {
        return Some(JournalType::DailyNotes);
    }
    if lower.contains("/meetings/") || lower.contains("/meeting/") {
        return Some(JournalType::Meeting);
    }
    if lower.contains("/notes/") || lower.contains("/note/") {
        return Some(JournalType::Note);
    }
    None
}

/// Extract a `YYYY-MM-DD` date from the start of a filename. Ports
/// `extractDateFromFilename`.
pub fn extract_date_from_filename(path: &Path) -> Option<NaiveDate> {
    let name = path.file_name()?.to_str()?;
    let bytes = name.as_bytes();
    // Expect at least `YYYY-MM-DD` (10 chars) with dashes at indices 4 and 7.
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    let (y, m, d) = (&name[0..4], &name[5..7], &name[8..10]);
    if !digits(y) || !digits(m) || !digits(d) {
        return None;
    }
    NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

/// A journal entry with metadata, mirroring the `JournalEntry` interface in list.ts.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub file_path: PathBuf,
    pub relative_path: String,
    pub ty: Option<JournalType>,
    pub date: Option<NaiveDate>,
    pub size: u64,
}

/// How to sort listed entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Date,
    Name,
}

/// Collect entry metadata for a set of files, relative to `base_dir`. Ports
/// `collectJournalEntries`; files that cannot be stat'd are skipped.
pub fn collect_journal_entries(files: &[PathBuf], base_dir: &Path) -> Vec<JournalEntry> {
    let mut entries = Vec::new();
    for file_path in files {
        let size = match std::fs::metadata(file_path) {
            Ok(m) => m.len(),
            Err(_) => continue,
        };
        let relative_path =
            file_path.strip_prefix(base_dir).unwrap_or(file_path).to_string_lossy().to_string();
        entries.push(JournalEntry {
            file_path: file_path.clone(),
            relative_path,
            ty: infer_type_from_path(file_path),
            date: extract_date_from_filename(file_path),
            size,
        });
    }
    entries
}

/// Filter by type, sort, and truncate to `limit`. Ports `filterAndSortEntries`.
pub fn filter_and_sort_entries(
    mut entries: Vec<JournalEntry>,
    ty: Option<JournalType>,
    limit: usize,
    sort: SortBy,
) -> Vec<JournalEntry> {
    if let Some(ty) = ty {
        entries.retain(|e| e.ty == Some(ty));
    }
    match sort {
        // Newest first; entries without a date sort last (TS treats them as epoch 0).
        SortBy::Date => entries.sort_by_key(|e| std::cmp::Reverse(e.date)),
        SortBy::Name => entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path)),
    }
    entries.truncate(limit);
    entries
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// A single search hit with surrounding context lines. Mirrors `SearchMatch`.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub file_path: PathBuf,
    pub line_number: usize,
    pub line: String,
    pub context: Vec<String>,
}

/// Options for [`search_journal_files`], mirroring `SearchOptions`.
pub struct SearchOptions<'a> {
    pub query: &'a str,
    pub ty: Option<JournalType>,
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub context_lines: usize,
}

impl Default for SearchOptions<'_> {
    fn default() -> Self {
        Self { query: "", ty: None, start_date: None, end_date: None, context_lines: 2 }
    }
}

/// Case-insensitive substring search over `files` with type/date filters and context
/// lines. Ports `searchJournalFiles`.
pub fn search_journal_files(files: &[PathBuf], options: &SearchOptions) -> Vec<SearchMatch> {
    let lower_query = options.query.to_lowercase();
    let mut matches = Vec::new();

    for file_path in files {
        if let Some(ty) = options.ty
            && infer_type_from_path(file_path) != Some(ty)
        {
            continue;
        }

        // Date-range filter only applies when the filename encodes a date.
        if (options.start_date.is_some() || options.end_date.is_some())
            && let Some(file_date) = extract_date_from_filename(file_path)
        {
            if let Some(start) = options.start_date
                && file_date < start
            {
                continue;
            }
            if let Some(end) = options.end_date
                && file_date > end
            {
                continue;
            }
        }

        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            if line.to_lowercase().contains(&lower_query) {
                let ctx_start = i.saturating_sub(options.context_lines);
                let ctx_end = (i + options.context_lines).min(lines.len() - 1);
                let mut context = Vec::new();
                for (j, ctx_line) in lines.iter().enumerate().take(ctx_end + 1).skip(ctx_start) {
                    let prefix = if j == i { ">" } else { " " };
                    context.push(format!("{prefix} {}: {}", j + 1, ctx_line));
                }
                matches.push(SearchMatch {
                    file_path: file_path.clone(),
                    line_number: i + 1,
                    line: (*line).to_string(),
                    context,
                });
            }
        }
    }

    matches
}

/// Parse a `YYYY-MM-DD..YYYY-MM-DD` range. Ports `parseDateRange`.
pub fn parse_date_range(range: &str) -> Result<(NaiveDate, NaiveDate)> {
    let parts: Vec<&str> = range.split("..").collect();
    if parts.len() != 2 {
        return Err(Error::InvalidDateRange(range.to_string()));
    }
    let start = NaiveDate::parse_from_str(parts[0], "%Y-%m-%d")
        .map_err(|_| Error::InvalidDateRange(range.to_string()))?;
    let end = NaiveDate::parse_from_str(parts[1], "%Y-%m-%d")
        .map_err(|_| Error::InvalidDateRange(range.to_string()))?;
    Ok((start, end))
}

/// Format a byte size like `formatSize` in list.ts (`B` / `KB` / `MB`).
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.1}KB");
    }
    let mb = kb / 1024.0;
    format!("{mb:.1}MB")
}

/// The Monday used for a daily-notes entry's content, exposed for callers that need it.
pub fn monday_for(date: NaiveDate) -> NaiveDate {
    monday_of_week(date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;
    use tempfile::TempDir;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        ymd(y, mo, d).and_time(NaiveTime::from_hms_opt(h, mi, 0).unwrap())
    }

    #[test]
    fn daily_content_uses_builtin_when_no_template_dir() {
        let content = create_journal_content(ymd(2026, 6, 29), None);
        let expected = "# Journal for Week 2026-06-29\n\n## 2026-06-29 Monday\n\n## 2026-06-30 Tuesday\n\n## 2026-07-01 Wednesday\n\n## 2026-07-02 Thursday\n\n## 2026-07-03 Friday\n";
        assert_eq!(content, expected);
    }

    #[test]
    fn meeting_content_builtin_fallback_has_ts_trailing_spaces() {
        let content = create_meeting_content("Team Sync", dt(2026, 7, 1, 9, 5), None);
        assert!(content.starts_with("# Meeting: Team Sync\n"));
        assert!(content.contains("**Date:** 2026-07-01\n**Time:** 09:05\n"));
        assert!(content.contains("**Attendees:** \n")); // trailing space preserved
        assert!(content.contains("- [ ] \n"));
    }

    #[test]
    fn note_content_builtin_fallback() {
        let content = create_note_content("My Idea", dt(2026, 7, 1, 14, 30), None);
        assert_eq!(
            content,
            "# My Idea\n\n**Created:** 2026-07-01 14:30\n\n## Summary\n\n## Details\n\n## References\n"
        );
    }

    #[test]
    fn external_template_takes_precedence() {
        let dir = TempDir::new().unwrap();
        ensure_templates_exist(dir.path()).unwrap();
        // Overwrite the note template with a custom one.
        std::fs::write(dir.path().join("note.md"), "custom {title} @ {date}\n").unwrap();
        let content = create_note_content("Hi", dt(2026, 7, 1, 8, 0), Some(dir.path()));
        assert_eq!(content, "custom Hi @ 2026-07-01\n");
    }

    #[test]
    fn ensure_templates_writes_defaults_once() {
        let dir = TempDir::new().unwrap();
        ensure_templates_exist(dir.path()).unwrap();
        let daily = std::fs::read_to_string(dir.path().join("daily-notes.md")).unwrap();
        assert_eq!(daily, DEFAULT_DAILY_TEMPLATE);
        // The on-disk meeting template has NO trailing space after `**Attendees:**`.
        let meeting = std::fs::read_to_string(dir.path().join("meeting.md")).unwrap();
        assert!(meeting.contains("**Attendees:**\n"));
        assert!(!meeting.contains("**Attendees:** \n"));
    }

    #[test]
    fn classifies_by_directory() {
        assert_eq!(
            infer_type_from_path(Path::new("/b/journal/2026/06/daily-notes/x.md")),
            Some(JournalType::DailyNotes)
        );
        assert_eq!(
            infer_type_from_path(Path::new("/b/journal/2026/07/meetings/x.md")),
            Some(JournalType::Meeting)
        );
        assert_eq!(
            infer_type_from_path(Path::new("/b/journal/2026/07/notes/x.md")),
            Some(JournalType::Note)
        );
        assert_eq!(infer_type_from_path(Path::new("/b/random/x.md")), None);
    }

    #[test]
    fn extracts_date_from_filename() {
        assert_eq!(
            extract_date_from_filename(Path::new("/x/2026-03-15-note.md")),
            Some(ymd(2026, 3, 15))
        );
        assert_eq!(extract_date_from_filename(Path::new("/x/no-date.md")), None);
    }

    #[test]
    fn list_and_filter_flow() {
        let dir = TempDir::new().unwrap();
        let base = dir.path();
        let daily = base.join("journal/2026/06/daily-notes");
        let meetings = base.join("journal/2026/07/meetings");
        std::fs::create_dir_all(&daily).unwrap();
        std::fs::create_dir_all(&meetings).unwrap();
        std::fs::write(daily.join("2026-06-29-daily-notes.md"), "hello").unwrap();
        std::fs::write(meetings.join("2026-07-01-team-sync.md"), "world").unwrap();

        let files = collect_markdown_files(&base.join("journal"));
        assert_eq!(files.len(), 2);

        let entries = collect_journal_entries(&files, base);
        let daily_only = filter_and_sort_entries(
            entries.clone(),
            Some(JournalType::DailyNotes),
            20,
            SortBy::Date,
        );
        assert_eq!(daily_only.len(), 1);
        assert_eq!(daily_only[0].ty, Some(JournalType::DailyNotes));

        // Date sort puts the newer meeting entry first.
        let all = filter_and_sort_entries(entries, None, 20, SortBy::Date);
        assert_eq!(all[0].date, Some(ymd(2026, 7, 1)));
    }

    #[test]
    fn search_finds_content_with_context() {
        let dir = TempDir::new().unwrap();
        let notes = dir.path().join("journal/2026/07/notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("2026-07-01-idea.md"), "line one\nfind ME here\nline three")
            .unwrap();

        let files = collect_markdown_files(&dir.path().join("journal"));
        let matches =
            search_journal_files(&files, &SearchOptions { query: "me", ..Default::default() });
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_number, 2);
        assert!(matches[0].context.iter().any(|c| c.starts_with("> 2: find ME here")));
    }

    #[test]
    fn search_respects_type_and_date_range() {
        let dir = TempDir::new().unwrap();
        let notes = dir.path().join("journal/2026/07/notes");
        let meetings = dir.path().join("journal/2026/07/meetings");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&meetings).unwrap();
        std::fs::write(notes.join("2026-07-01-a.md"), "target").unwrap();
        std::fs::write(meetings.join("2026-07-10-b.md"), "target").unwrap();

        let files = collect_markdown_files(&dir.path().join("journal"));

        let notes_only = search_journal_files(
            &files,
            &SearchOptions { query: "target", ty: Some(JournalType::Note), ..Default::default() },
        );
        assert_eq!(notes_only.len(), 1);

        let (start, end) = parse_date_range("2026-07-05..2026-07-15").unwrap();
        let ranged = search_journal_files(
            &files,
            &SearchOptions {
                query: "target",
                start_date: Some(start),
                end_date: Some(end),
                ..Default::default()
            },
        );
        assert_eq!(ranged.len(), 1); // only the 07-10 meeting falls in range
    }

    #[test]
    fn parse_date_range_rejects_bad_input() {
        assert!(parse_date_range("2026-01-01").is_err());
        assert!(parse_date_range("bad..worse").is_err());
    }

    #[test]
    fn format_size_thresholds() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(2048), "2.0KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0MB");
    }
}
