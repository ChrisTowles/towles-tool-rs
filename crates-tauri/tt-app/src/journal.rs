//! Tauri bridge for the journal screens (`tt_journal`): today's daily note, listing
//! and searching entries, creating notes/meetings, and opening any entry in the
//! preferred editor. Mirrors the `tt journal` CLI boundary in
//! `crates-cli/tt-cli/src/commands/journal.rs`, minus the editor-open-by-default and
//! interactive-title-prompt behavior (the UI drives those explicitly).

use std::path::{Path, PathBuf};

use serde::Serialize;

use tt_journal::JournalType;
use tt_journal::entries::{self, SortBy};
use tt_journal::tokens::generate_journal_file_info;

fn load_settings() -> Result<tt_config::UserSettings, String> {
    tt_config::load().map_err(|e| format!("failed to load settings: {e}"))
}

fn parse_type(raw: Option<&str>) -> Result<Option<JournalType>, String> {
    match raw {
        None => Ok(None),
        Some(v) => JournalType::parse(v).map(Some).map_err(|e| e.to_string()),
    }
}

fn relative_to(base: &Path, path: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).to_string_lossy().to_string()
}

/// Today's daily note, created from the template if it doesn't exist yet.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodayNote {
    pub relative_path: String,
    pub content: String,
}

#[tauri::command]
pub fn journal_get_today() -> Result<TodayNote, String> {
    let settings = load_settings()?;
    let journal = &settings.journal_settings;
    let template_dir = Path::new(&journal.template_dir);
    entries::ensure_templates_exist(template_dir).map_err(|e| e.to_string())?;

    let today = chrono::Local::now().date_naive();
    let info = generate_journal_file_info(journal, today, JournalType::DailyNotes, "");
    if let Some(parent) = info.full_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if !info.full_path.exists() {
        let content = entries::create_journal_content(info.monday_date, Some(template_dir));
        std::fs::write(&info.full_path, content).map_err(|e| e.to_string())?;
    }

    let content = std::fs::read_to_string(&info.full_path).map_err(|e| e.to_string())?;
    let base_folder = PathBuf::from(&journal.base_folder);
    Ok(TodayNote { relative_path: relative_to(&base_folder, &info.full_path), content })
}

/// Replace the full content of a journal entry (path relative to the base folder).
///
/// `expected_original` is the content the UI last loaded; the save refuses to overwrite
/// (returning an error the UI surfaces as "file changed on disk") when the file has
/// changed since then, so a concurrent `journal_log` append or external edit is not
/// silently lost. The underlying write is atomic (temp file + rename).
#[tauri::command]
pub fn journal_save(
    relative_path: String,
    expected_original: String,
    content: String,
) -> Result<(), String> {
    let settings = load_settings()?;
    let full_path = PathBuf::from(&settings.journal_settings.base_folder).join(&relative_path);
    tt_journal::save::save_file(&full_path, &expected_original, &content).map_err(|e| e.to_string())
}

/// A listed journal entry, mirroring `entries::JournalEntry` minus the absolute path.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntryDto {
    pub relative_path: String,
    pub ty: Option<String>,
    pub date: Option<String>,
    pub size_label: String,
}

#[tauri::command]
pub fn journal_list(
    ty: Option<String>,
    limit: Option<usize>,
    sort: Option<String>,
) -> Result<Vec<JournalEntryDto>, String> {
    let settings = load_settings()?;
    let base_folder = PathBuf::from(&settings.journal_settings.base_folder);
    let journal_dir = base_folder.join("journal");

    let type_filter = parse_type(ty.as_deref())?;
    let sort = match sort.as_deref() {
        None | Some("date") => SortBy::Date,
        Some("name") => SortBy::Name,
        Some(other) => return Err(format!("invalid sort \"{other}\". Must be one of: date, name")),
    };

    let files = entries::collect_markdown_files(&journal_dir);
    let all = entries::collect_journal_entries(&files, &base_folder);
    let result = entries::filter_and_sort_entries(all, type_filter, limit.unwrap_or(50), sort);

    Ok(result
        .into_iter()
        .map(|e| JournalEntryDto {
            relative_path: e.relative_path,
            ty: e.ty.map(|t| t.as_str().to_string()),
            date: e.date.map(entries::format_date),
            size_label: entries::format_size(e.size),
        })
        .collect())
}

/// A single search hit, mirroring `entries::SearchMatch` minus the absolute path.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatchDto {
    pub relative_path: String,
    pub line_number: usize,
    pub context: Vec<String>,
}

#[tauri::command]
pub fn journal_search(
    query: String,
    ty: Option<String>,
    range: Option<String>,
) -> Result<Vec<SearchMatchDto>, String> {
    let settings = load_settings()?;
    let base_folder = PathBuf::from(&settings.journal_settings.base_folder);
    let journal_dir = base_folder.join("journal");

    let type_filter = parse_type(ty.as_deref())?;
    let (start_date, end_date) = match range {
        Some(raw) => {
            let (s, e) = entries::parse_date_range(&raw).map_err(|e| e.to_string())?;
            (Some(s), Some(e))
        }
        None => (None, None),
    };

    let files = entries::collect_markdown_files(&journal_dir);
    let matches = entries::search_journal_files(
        &files,
        &entries::SearchOptions {
            query: &query,
            ty: type_filter,
            start_date,
            end_date,
            context_lines: 2,
        },
    );

    Ok(matches
        .into_iter()
        .map(|m| SearchMatchDto {
            relative_path: relative_to(&base_folder, &m.file_path),
            line_number: m.line_number,
            context: m.context,
        })
        .collect())
}

/// Create a new note or meeting with `title` (daily notes are created automatically
/// by `journal_get_today`), returning its path relative to the journal base folder.
#[tauri::command]
pub fn journal_create(ty: String, title: String) -> Result<String, String> {
    let settings = load_settings()?;
    let journal = &settings.journal_settings;
    let journal_ty = JournalType::parse(&ty).map_err(|e| e.to_string())?;
    if journal_ty == JournalType::DailyNotes {
        return Err("daily notes are created automatically".into());
    }
    if title.trim().is_empty() {
        return Err("title is required".into());
    }

    let template_dir = Path::new(&journal.template_dir);
    entries::ensure_templates_exist(template_dir).map_err(|e| e.to_string())?;

    let now = chrono::Local::now();
    let info = generate_journal_file_info(journal, now.date_naive(), journal_ty, &title);
    if let Some(parent) = info.full_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if !info.full_path.exists() {
        let dt = now.naive_local();
        let content = match journal_ty {
            JournalType::Meeting => entries::create_meeting_content(&title, dt, Some(template_dir)),
            _ => entries::create_note_content(&title, dt, Some(template_dir)),
        };
        std::fs::write(&info.full_path, content).map_err(|e| e.to_string())?;
    }

    let base_folder = PathBuf::from(&journal.base_folder);
    Ok(relative_to(&base_folder, &info.full_path))
}

/// Open a journal entry (path relative to the base folder) in the preferred
/// editor. Spawns without waiting (like `ab_open_in_editor`): `tt_exec::run`
/// waits for the process to exit, which froze the app for the whole editor
/// session with any non-forking editor (vim, `code --wait`).
#[tauri::command]
pub fn journal_open(relative_path: String) -> Result<(), String> {
    let settings = load_settings()?;
    let journal = &settings.journal_settings;
    let editor = settings.preferred_editor.trim();
    if editor.is_empty() {
        return Err("No preferred editor configured".into());
    }
    let full_path = PathBuf::from(&journal.base_folder).join(&relative_path);
    tt_exec::record_detached_spawn(
        editor,
        &[&journal.base_folder, &full_path.to_string_lossy()],
        "editor",
    );
    std::process::Command::new(editor)
        .arg(&journal.base_folder)
        .arg(&full_path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open in editor: {e}"))
}
