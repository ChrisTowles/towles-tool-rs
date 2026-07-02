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
