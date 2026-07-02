//! `tt journal` subcommands: daily-notes, note, meeting, list, search.
//!
//! Ports `src/commands/journal/*.ts`. Library logic lives in `tt-journal`; this module
//! is the CLI boundary — it loads settings, resolves the current date, calls the library,
//! and flattens errors to exit codes (the yaak pattern).
//!
//! Deviations from the TS CLI (see docs/MIGRATION.md):
//! - A `--no-open` flag suppresses the editor; the editor is also skipped when stdout is
//!   not a TTY, so tests and CI never spawn one.
//! - The per-command `--debug` flag is replaced by the global `-v/--verbose` flag.

use crate::cli::JournalCommands;
use crate::ui;
use chrono::Local;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tt_config::UserSettings;
use tt_journal::JournalType;
use tt_journal::entries::{self, SearchOptions, SortBy};
use tt_journal::tokens::generate_journal_file_info;

/// Resolve the settings file path, honoring a `--config-dir` override.
fn resolve_config_path(config_dir: Option<&Path>) -> Result<PathBuf, tt_config::Error> {
    match config_dir {
        Some(dir) => Ok(dir.join(format!("{}.settings.json", tt_config::TOOL_NAME))),
        None => tt_config::config_path(),
    }
}

fn load_settings(config_dir: Option<&Path>) -> Result<UserSettings, String> {
    let path = resolve_config_path(config_dir)
        .map_err(|e| format!("Could not resolve config path: {e}"))?;
    tt_config::load_from(&path).map_err(|e| format!("Failed to load settings: {e}"))
}

pub fn run(command: JournalCommands, config_dir: Option<&Path>) -> i32 {
    match command {
        JournalCommands::DailyNotes { no_open } => daily_notes(config_dir, no_open),
        JournalCommands::Note { title, no_open } => {
            note_like(config_dir, JournalType::Note, title, no_open)
        }
        JournalCommands::Meeting { title, no_open } => {
            note_like(config_dir, JournalType::Meeting, title, no_open)
        }
        JournalCommands::List { r#type, limit, sort } => list(config_dir, r#type, limit, sort),
        JournalCommands::Search { query, r#type, range } => {
            search(config_dir, query, r#type, range)
        }
    }
}

fn daily_notes(config_dir: Option<&Path>, no_open: bool) -> i32 {
    let settings = match load_settings(config_dir) {
        Ok(s) => s,
        Err(e) => {
            ui::error(&e);
            return 1;
        }
    };
    let journal = &settings.journal_settings;
    let template_dir = Path::new(&journal.template_dir);

    if let Err(e) = entries::ensure_templates_exist(template_dir) {
        ui::warning(&format!("Could not create templates: {e}"));
    }

    let today = Local::now().date_naive();
    let info = generate_journal_file_info(journal, today, JournalType::DailyNotes, "");

    if let Err(e) = ensure_parent(&info.full_path) {
        ui::error(&format!("Error creating daily-notes file: {e}"));
        return 1;
    }

    if info.full_path.exists() {
        ui::info(&format!("Opening existing daily-notes file: {}", info.full_path.display()));
    } else {
        let content = entries::create_journal_content(info.monday_date, Some(template_dir));
        ui::info(&format!("Creating new daily-notes file: {}", info.full_path.display()));
        if let Err(e) = std::fs::write(&info.full_path, content) {
            ui::error(&format!("Error creating daily-notes file: {e}"));
            return 1;
        }
    }

    open_in_editor(&settings.preferred_editor, &journal.base_folder, &info.full_path, no_open);
    0
}

fn note_like(
    config_dir: Option<&Path>,
    ty: JournalType,
    title: Option<String>,
    no_open: bool,
) -> i32 {
    let settings = match load_settings(config_dir) {
        Ok(s) => s,
        Err(e) => {
            ui::error(&e);
            return 1;
        }
    };
    let journal = &settings.journal_settings;
    let template_dir = Path::new(&journal.template_dir);

    if let Err(e) = entries::ensure_templates_exist(template_dir) {
        ui::warning(&format!("Could not create templates: {e}"));
    }

    let label = if ty == JournalType::Meeting { "meeting" } else { "note" };
    let title = match resolve_title(title, label) {
        Ok(t) => t,
        Err(e) => {
            ui::error(&e);
            return 1;
        }
    };

    let now = Local::now();
    let info = generate_journal_file_info(journal, now.date_naive(), ty, &title);

    if let Err(e) = ensure_parent(&info.full_path) {
        ui::error(&format!("Error creating {label} file: {e}"));
        return 1;
    }

    if info.full_path.exists() {
        ui::info(&format!("Opening existing {label} file: {}", info.full_path.display()));
    } else {
        let dt = now.naive_local();
        let content = match ty {
            JournalType::Meeting => entries::create_meeting_content(&title, dt, Some(template_dir)),
            _ => entries::create_note_content(&title, dt, Some(template_dir)),
        };
        ui::info(&format!("Creating new {label} file: {}", info.full_path.display()));
        if let Err(e) = std::fs::write(&info.full_path, content) {
            ui::error(&format!("Error creating {label} file: {e}"));
            return 1;
        }
    }

    open_in_editor(&settings.preferred_editor, &journal.base_folder, &info.full_path, no_open);
    0
}

/// Resolve a title: use the argument, else prompt interactively (matching the TS CLI).
/// When stdin is not a terminal, prompting is impossible, so we fail with a clear error.
fn resolve_title(title: Option<String>, label: &str) -> Result<String, String> {
    let title = title.unwrap_or_default();
    if !title.trim().is_empty() {
        return Ok(title);
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!("A {label} title is required (pass it as an argument)."));
    }
    inquire::Text::new(&format!("Enter {label} title:"))
        .prompt()
        .map_err(|e| format!("Could not read {label} title: {e}"))
}

fn list(
    config_dir: Option<&Path>,
    ty: Option<String>,
    limit: Option<String>,
    sort: Option<String>,
) -> i32 {
    let settings = match load_settings(config_dir) {
        Ok(s) => s,
        Err(e) => {
            ui::error(&e);
            return 1;
        }
    };
    let base_folder = PathBuf::from(&settings.journal_settings.base_folder);
    let journal_dir = base_folder.join("journal");

    let type_filter = match parse_type(ty.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            ui::error(&e);
            return 1;
        }
    };

    let limit = match limit {
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => {
                ui::error(&format!("Invalid limit \"{raw}\". Must be a positive integer."));
                return 1;
            }
        },
        None => 20,
    };

    let sort = match sort.as_deref() {
        None | Some("date") => SortBy::Date,
        Some("name") => SortBy::Name,
        Some(other) => {
            ui::error(&format!("Invalid sort \"{other}\". Must be one of: date, name"));
            return 1;
        }
    };

    let files = entries::collect_markdown_files(&journal_dir);
    if files.is_empty() {
        ui::info(&format!("No journal files found in {}", journal_dir.display()));
        return 0;
    }

    let all = entries::collect_journal_entries(&files, &base_folder);
    let result = entries::filter_and_sort_entries(all, type_filter, limit, sort);
    if result.is_empty() {
        ui::info("No matching journal entries found.");
        return 0;
    }

    ui::info(&format!("Showing {} journal entries:", result.len()));
    println!();
    println!("{}", render_table(&result));
    0
}

fn search(
    config_dir: Option<&Path>,
    query: String,
    ty: Option<String>,
    range: Option<String>,
) -> i32 {
    let settings = match load_settings(config_dir) {
        Ok(s) => s,
        Err(e) => {
            ui::error(&e);
            return 1;
        }
    };
    let base_folder = PathBuf::from(&settings.journal_settings.base_folder);
    let journal_dir = base_folder.join("journal");

    let type_filter = match parse_type(ty.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            ui::error(&e);
            return 1;
        }
    };

    let (start_date, end_date) = match range {
        Some(raw) => match entries::parse_date_range(&raw) {
            Ok((s, e)) => (Some(s), Some(e)),
            Err(e) => {
                ui::error(&e.to_string());
                return 1;
            }
        },
        None => (None, None),
    };

    let files = entries::collect_markdown_files(&journal_dir);
    if files.is_empty() {
        ui::info(&format!("No journal files found in {}", journal_dir.display()));
        return 0;
    }

    let matches = entries::search_journal_files(
        &files,
        &SearchOptions { query: &query, ty: type_filter, start_date, end_date, context_lines: 2 },
    );

    if matches.is_empty() {
        ui::info(&format!("No matches found for \"{query}\""));
        return 0;
    }

    ui::info(&format!("Found {} match(es) for \"{query}\":", matches.len()));
    println!();

    // Group matches by file, preserving first-seen order.
    let mut order: Vec<PathBuf> = Vec::new();
    let mut by_file: std::collections::HashMap<PathBuf, Vec<&tt_journal::entries::SearchMatch>> =
        std::collections::HashMap::new();
    for m in &matches {
        if !by_file.contains_key(&m.file_path) {
            order.push(m.file_path.clone());
        }
        by_file.entry(m.file_path.clone()).or_default().push(m);
    }

    for file in order {
        let relative = file.strip_prefix(&base_folder).unwrap_or(&file);
        println!("{}", relative.display());
        for m in &by_file[&file] {
            for line in &m.context {
                println!("{line}");
            }
            println!();
        }
    }
    0
}

fn parse_type(raw: Option<&str>) -> Result<Option<JournalType>, String> {
    match raw {
        None => Ok(None),
        Some(v) => JournalType::parse(v).map(Some).map_err(|e| e.to_string()),
    }
}

fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Open `file` (with `folder` for editor context) unless suppressed. Mirrors
/// `openInEditor`: run `<editor> <folder> <file>` and warn (don't fail) if it can't run.
fn open_in_editor(editor: &str, folder: &str, file: &Path, no_open: bool) {
    if no_open || !std::io::stdout().is_terminal() {
        return;
    }
    let file_str = file.to_string_lossy();
    if let Err(e) = tt_exec::run(editor, &[folder, &file_str]) {
        ui::warning(&format!(
            "Could not open in editor '{editor}'. Set 'preferredEditor' in the config (e.g. 'code', 'code-insiders'). {e}"
        ));
    }
}

/// Render entries as a padded table. Ports `renderTable` in list.ts.
fn render_table(entries: &[entries::JournalEntry]) -> String {
    struct Row {
        file: String,
        ty: String,
        date: String,
        size: String,
    }
    let mut rows = vec![Row {
        file: "FILE".to_string(),
        ty: "TYPE".to_string(),
        date: "DATE".to_string(),
        size: "SIZE".to_string(),
    }];
    for e in entries {
        rows.push(Row {
            file: e.relative_path.clone(),
            ty: e.ty.map(|t| t.as_str().to_string()).unwrap_or_else(|| "unknown".to_string()),
            date: e.date.map(entries::format_date).unwrap_or_else(|| "-".to_string()),
            size: entries::format_size(e.size),
        });
    }
    let w_file = rows.iter().map(|r| r.file.len()).max().unwrap_or(0);
    let w_ty = rows.iter().map(|r| r.ty.len()).max().unwrap_or(0);
    let w_date = rows.iter().map(|r| r.date.len()).max().unwrap_or(0);
    let w_size = rows.iter().map(|r| r.size.len()).max().unwrap_or(0);

    rows.iter()
        .map(|r| {
            format!(
                "{:<wf$}  {:<wt$}  {:<wd$}  {:>ws$}",
                r.file,
                r.ty,
                r.date,
                r.size,
                wf = w_file,
                wt = w_ty,
                wd = w_date,
                ws = w_size,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
