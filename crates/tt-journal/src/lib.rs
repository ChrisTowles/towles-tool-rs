//! Journal and note-taking library for the towles-tool CLI.
//!
//! Ports `src/commands/journal/` from the TypeScript CLI. The two public modules mirror
//! the split in the original:
//!
//! - [`tokens`] — path-template rendering (Luxon-style `{yyyy}`/`{monday:...}` tokens),
//!   Monday-of-week math, and title slugification. Output must match the TS CLI
//!   byte-for-byte because both tools share the same settings file.
//! - [`entries`] — template scaffolding, journal content creation, listing, and search.
//!
//! Both CLIs read the same settings file, so this crate is deliberately Tauri-free and
//! depends only on [`tt_config`] for the settings model plus `chrono` for date math.

use thiserror::Error;

pub mod entries;
pub mod tokens;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid entry type \"{0}\". Must be one of: daily-notes, meeting, note")]
    InvalidType(String),

    #[error("Invalid date range format: \"{0}\". Expected: YYYY-MM-DD..YYYY-MM-DD")]
    InvalidDateRange(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The three kinds of journal entry, mirroring `JOURNAL_TYPES` in the TS CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalType {
    DailyNotes,
    Meeting,
    Note,
}

impl JournalType {
    /// The wire string used in paths, filters, and CLI output (e.g. `"daily-notes"`).
    pub fn as_str(self) -> &'static str {
        match self {
            JournalType::DailyNotes => "daily-notes",
            JournalType::Meeting => "meeting",
            JournalType::Note => "note",
        }
    }

    /// Parse a `--type` filter value. Mirrors `parseTypeFilter` in `search.ts`.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "daily-notes" => Ok(JournalType::DailyNotes),
            "meeting" => Ok(JournalType::Meeting),
            "note" => Ok(JournalType::Note),
            other => Err(Error::InvalidType(other.to_string())),
        }
    }
}
