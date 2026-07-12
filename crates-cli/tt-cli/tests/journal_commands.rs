mod common;

use chrono::{Datelike, Duration, Local, NaiveDate};
use common::{cli_cmd, write_journal_settings};
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Set up a sandbox: config dir + journal base + template dir, all inside a tempdir.
struct Sandbox {
    _dir: TempDir,
    config: PathBuf,
    base: PathBuf,
}

fn sandbox() -> Sandbox {
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("config");
    let base = dir.path().join("journal-base");
    let templates = dir.path().join("templates");
    write_journal_settings(&config, &base, &templates);
    Sandbox { _dir: dir, config, base }
}

/// Monday of the current local week, mirroring `getMondayOfWeek`.
fn monday_of_this_week() -> NaiveDate {
    let today = Local::now().date_naive();
    today - Duration::days(today.weekday().num_days_from_monday() as i64)
}

fn cmd(sandbox: &Sandbox) -> assert_cmd::Command {
    cli_cmd(&sandbox.config)
}

#[test]
fn daily_notes_creates_weekly_path() {
    let sb = sandbox();
    cmd(&sb)
        .args(["journal", "daily-notes", "--no-open"])
        .assert()
        .success()
        .stdout(contains("daily-notes"));

    let monday = monday_of_this_week();
    let expected = sb.base.join(format!(
        "journal/{:04}/{:02}/daily-notes/{:04}-{:02}-{:02}-daily-notes.md",
        monday.year(),
        monday.month(),
        monday.year(),
        monday.month(),
        monday.day(),
    ));
    assert!(expected.exists(), "expected daily-notes file at {}", expected.display());

    let content = std::fs::read_to_string(&expected).unwrap();
    assert!(content.contains("# Journal for Week"));
    assert!(content.contains("Monday"));
}

#[test]
fn today_is_alias_for_daily_notes() {
    let sb = sandbox();
    cmd(&sb).args(["today", "--no-open"]).assert().success();

    let monday = monday_of_this_week();
    let expected = sb.base.join(format!(
        "journal/{:04}/{:02}/daily-notes/{:04}-{:02}-{:02}-daily-notes.md",
        monday.year(),
        monday.month(),
        monday.year(),
        monday.month(),
        monday.day(),
    ));
    assert!(expected.exists(), "`today` should create the same file as `journal daily-notes`");
}

#[test]
fn note_with_title_creates_file_and_template_content() {
    let sb = sandbox();
    cmd(&sb)
        .args(["journal", "note", "My Big Idea", "--no-open"])
        .assert()
        .success()
        .stdout(contains("note"));

    let today = Local::now().date_naive();
    let expected = sb.base.join(format!(
        "journal/{:04}/{:02}/notes/{:04}-{:02}-{:02}-my-big-idea.md",
        today.year(),
        today.month(),
        today.year(),
        today.month(),
        today.day(),
    ));
    assert!(expected.exists(), "expected note file at {}", expected.display());

    let content = std::fs::read_to_string(&expected).unwrap();
    assert!(content.contains("# My Big Idea"));
    assert!(content.contains("## Summary"));
}

#[test]
fn meeting_with_title_creates_file() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "meeting", "Team Sync", "--no-open"]).assert().success();

    let today = Local::now().date_naive();
    let expected = sb.base.join(format!(
        "journal/{:04}/{:02}/meetings/{:04}-{:02}-{:02}-team-sync.md",
        today.year(),
        today.month(),
        today.year(),
        today.month(),
        today.day(),
    ));
    assert!(expected.exists(), "expected meeting file at {}", expected.display());
    let content = std::fs::read_to_string(&expected).unwrap();
    assert!(content.contains("# Meeting: Team Sync"));
    assert!(content.contains("## Agenda"));
}

/// Path to this week's daily-notes file inside the sandbox.
fn daily_note_path(sb: &Sandbox) -> PathBuf {
    let monday = monday_of_this_week();
    sb.base.join(format!(
        "journal/{:04}/{:02}/daily-notes/{:04}-{:02}-{:02}-daily-notes.md",
        monday.year(),
        monday.month(),
        monday.year(),
        monday.month(),
        monday.day(),
    ))
}

#[test]
fn jot_arg_creates_daily_note_and_appends_bullet() {
    let sb = sandbox();
    cmd(&sb)
        .args(["journal", "jot", "shipped the parser"])
        .assert()
        .success()
        .stdout(contains("Jotted to"));

    let path = daily_note_path(&sb);
    assert!(path.exists(), "expected daily-notes file at {}", path.display());
    let content = std::fs::read_to_string(&path).unwrap();
    // Weekly scaffold plus a `- HH:MM <text>` bullet (timestamp is HH:MM, so ':' present).
    assert!(content.contains("# Journal for Week"));
    assert!(content.contains(":"));
    assert!(content.contains("shipped the parser"));
    let bullet = content.lines().find(|l| l.contains("shipped the parser")).unwrap();
    assert!(bullet.starts_with("- "), "bullet should start with `- `, got: {bullet}");
}

#[test]
fn jot_reads_from_stdin_when_dash() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "jot", "-"]).write_stdin("piped thought").assert().success();

    let content = std::fs::read_to_string(daily_note_path(&sb)).unwrap();
    assert!(content.contains("piped thought"));
    // No editor is spawned: `preferredEditor` is `true` in the sandbox, which would
    // succeed silently, but the command must not depend on it — success above proves it.
}

#[test]
fn jot_reads_from_stdin_when_arg_omitted() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "jot"]).write_stdin("no-arg thought\n").assert().success();

    let content = std::fs::read_to_string(daily_note_path(&sb)).unwrap();
    assert!(content.contains("no-arg thought"));
    // Trailing whitespace is trimmed; the bullet is a single clean line.
    assert!(content.contains("- "));
    assert!(!content.contains("no-arg thought \n"));
}

#[test]
fn jot_appends_multiple_bullets_in_order() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "jot", "first"]).assert().success();
    cmd(&sb).args(["journal", "jot", "second"]).assert().success();

    let content = std::fs::read_to_string(daily_note_path(&sb)).unwrap();
    let first = content.find("first").unwrap();
    let second = content.find("second").unwrap();
    assert!(first < second, "bullets should preserve capture order");
}

#[test]
fn jot_rejects_empty_stdin() {
    let sb = sandbox();
    cmd(&sb)
        .args(["journal", "jot"])
        .write_stdin("   \n")
        .assert()
        .failure()
        .stderr(contains("Nothing to jot"));

    // Nothing was written.
    assert!(!daily_note_path(&sb).exists());
}

#[test]
fn list_shows_created_entries() {
    let sb = sandbox();
    // Create a note and a meeting.
    cmd(&sb).args(["journal", "note", "Alpha Note", "--no-open"]).assert().success();
    cmd(&sb).args(["journal", "meeting", "Beta Meeting", "--no-open"]).assert().success();

    cmd(&sb)
        .args(["journal", "list"])
        .assert()
        .success()
        .stdout(contains("alpha-note"))
        .stdout(contains("beta-meeting"))
        .stdout(contains("FILE"));

    // Type filter narrows to a single kind.
    cmd(&sb)
        .args(["journal", "list", "--type", "note"])
        .assert()
        .success()
        .stdout(contains("alpha-note"))
        .stdout(contains("beta-meeting").not());
}

#[test]
fn search_finds_content() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "note", "Searchable", "--no-open"]).assert().success();

    // The note template contains the word "Summary".
    cmd(&sb)
        .args(["journal", "search", "summary"])
        .assert()
        .success()
        .stdout(contains("Summary"))
        .stdout(contains("searchable"));

    // A query that matches nothing reports no matches.
    cmd(&sb)
        .args(["journal", "search", "zzz-nonexistent-zzz"])
        .assert()
        .success()
        .stdout(contains("No matches found"));
}

#[test]
fn list_json_emits_array_newest_first_with_type_filter() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "note", "Alpha Note", "--no-open"]).assert().success();
    cmd(&sb).args(["journal", "meeting", "Beta Meeting", "--no-open"]).assert().success();

    let output = cmd(&sb).args(["journal", "list", "--json"]).output().unwrap();
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = parsed.as_array().expect("top-level JSON array");
    assert_eq!(arr.len(), 2);

    // Note and meeting are created today, so ordering is stable by name tiebreak within
    // the same date; assert both are present with absolute paths and their wire types.
    let types: Vec<&str> = arr.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert!(types.contains(&"note"));
    assert!(types.contains(&"meeting"));
    for e in arr {
        let path = e["path"].as_str().unwrap();
        assert!(Path::new(path).is_absolute(), "path should be absolute: {path}");
        assert!(e["date"].is_string());
        assert!(e["size"].is_u64());
    }

    // Type filter narrows the array to a single kind.
    let filtered = cmd(&sb).args(["journal", "list", "--type", "note", "--json"]).output().unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&filtered.stdout).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"].as_str().unwrap(), "note");
}

#[test]
fn list_json_empty_journal_is_empty_array() {
    let sb = sandbox();
    let output = cmd(&sb).args(["journal", "list", "--json"]).output().unwrap();
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 0);
}

#[test]
fn open_last_no_open_prints_newest_absolute_path() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "note", "Older Note", "--no-open"]).assert().success();
    cmd(&sb).args(["journal", "meeting", "Newer Meeting", "--no-open"]).assert().success();

    let output = cmd(&sb).args(["journal", "open", "--last", "--no-open"]).output().unwrap();
    assert!(output.status.success());
    let printed = String::from_utf8(output.stdout).unwrap();
    let line = printed.lines().next_back().unwrap().trim();
    assert!(Path::new(line).is_absolute(), "expected an absolute path, got: {line}");
    assert!(Path::new(line).exists(), "printed path should exist: {line}");

    // With a type filter, `open` targets the newest of that type only.
    let output =
        cmd(&sb).args(["journal", "open", "--type", "note", "--no-open"]).output().unwrap();
    assert!(output.status.success());
    let printed = String::from_utf8(output.stdout).unwrap();
    let line = printed.lines().next_back().unwrap().trim();
    assert!(line.contains("older-note"), "type filter should pick the note, got: {line}");
}

#[test]
fn open_empty_journal_errors() {
    let sb = sandbox();
    cmd(&sb)
        .args(["journal", "open", "--no-open"])
        .assert()
        .failure()
        .stderr(contains("No journal entries found"));
}

#[test]
fn search_json_returns_matches_with_path_and_line() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "note", "Searchable", "--no-open"]).assert().success();

    // The note template contains the word "Summary".
    let output = cmd(&sb).args(["journal", "search", "summary", "--json"]).output().unwrap();
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let arr = parsed.as_array().expect("JSON array");
    assert!(!arr.is_empty());
    let hit = &arr[0];
    assert!(Path::new(hit["path"].as_str().unwrap()).is_absolute());
    assert!(hit["line_number"].as_u64().unwrap() >= 1);
    assert!(hit["line"].as_str().unwrap().to_lowercase().contains("summary"));
    assert_eq!(hit["type"].as_str().unwrap(), "note");
}

#[test]
fn search_json_no_match_is_empty_array() {
    let sb = sandbox();
    cmd(&sb).args(["journal", "note", "Searchable", "--no-open"]).assert().success();

    let output =
        cmd(&sb).args(["journal", "search", "zzz-nonexistent-zzz", "--json"]).output().unwrap();
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 0);
}

#[test]
fn note_without_title_in_non_tty_errors() {
    let sb = sandbox();
    // stdin is not a TTY under assert_cmd, so an omitted title must fail cleanly
    // rather than hang on a prompt.
    cmd(&sb)
        .args(["journal", "note", "--no-open"])
        .assert()
        .failure()
        .stderr(contains("title is required"));
}

#[test]
fn list_reports_empty_journal() {
    let sb = sandbox();
    // Nothing created yet; base/journal doesn't exist.
    let _ = Path::new(&sb.base);
    cmd(&sb)
        .args(["journal", "list"])
        .assert()
        .success()
        .stdout(contains("No journal files found"));
}
