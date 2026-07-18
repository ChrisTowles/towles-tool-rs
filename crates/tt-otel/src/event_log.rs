//! The on-disk event-log sink: append-only JSONL, one record per line,
//! rotated per UTC day.
//!
//! JSONL rather than a binary or OTLP-native format because the whole point is
//! answering questions later with whatever is at hand — `jq`, `grep`, a
//! throwaway script — without standing up a collector first. Records use
//! OpenTelemetry attribute names (`service.name`, `process.pid`,
//! `process.command_args`) so a log can still be replayed into a real
//! collector if one ever exists.
//!
//! Per the crate's determinism rule the writer takes its directory and its
//! clock from the caller; nothing here reads `$HOME` or `SystemTime`.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

/// A day's log file, kept open until the date rolls over.
struct OpenDay {
    /// `%Y-%m-%d` the open handle belongs to, compared against each record's
    /// timestamp to detect a rollover.
    date: String,
    writer: BufWriter<File>,
}

/// Append-only JSONL writer with per-day rotation and count-based retention.
///
/// Every write is flushed. Telemetry is read *after* something went wrong —
/// often after a crash or a `kill -9` of a wedged process — so buffering the
/// most interesting records into oblivion would defeat the purpose. Records are
/// small and the volume is bounded by subprocess spawns, so the syscall cost is
/// not worth optimising against that.
pub struct EventLog {
    dir: PathBuf,
    open: Option<OpenDay>,
    /// Days of history to keep; older files are pruned on rollover.
    retain_days: usize,
}

impl EventLog {
    /// Create a log writing into `dir`, keeping `retain_days` of history.
    /// The directory is created on first write, not here, so constructing a
    /// log for a path that is never written leaves no trace on disk.
    pub fn new(dir: impl Into<PathBuf>, retain_days: usize) -> Self {
        Self { dir: dir.into(), open: None, retain_days: retain_days.max(1) }
    }

    /// Append one already-serialized record, stamping it at `now`.
    ///
    /// Errors are deliberately swallowed into a `bool`: a telemetry sink that
    /// can propagate failure into the code it observes is worse than one that
    /// silently misses records. Returns whether the record reached the file, so
    /// tests can assert on it.
    pub fn append(&mut self, record: &serde_json::Value, now: DateTime<Utc>) -> bool {
        let date = now.format("%Y-%m-%d").to_string();
        if self.open.as_ref().is_none_or(|open| open.date != date) {
            if !self.rotate_to(&date) {
                return false;
            }
            self.prune();
        }
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        let Ok(line) = serde_json::to_string(record) else {
            return false;
        };
        writeln!(open.writer, "{line}").is_ok() && open.writer.flush().is_ok()
    }

    /// Path of the file holding `date`'s records.
    fn path_for(&self, date: &str) -> PathBuf {
        self.dir.join(format!("events-{date}.jsonl"))
    }

    /// Open (creating if needed) the file for `date`, dropping any previous
    /// handle. Returns whether the handle is usable.
    fn rotate_to(&mut self, date: &str) -> bool {
        // Drop the old handle first so its buffer flushes before the new file
        // takes over.
        self.open = None;
        if fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        match OpenOptions::new().create(true).append(true).open(self.path_for(date)) {
            Ok(file) => {
                self.open = Some(OpenDay { date: date.to_string(), writer: BufWriter::new(file) });
                true
            }
            Err(_) => false,
        }
    }

    /// Delete all but the newest `retain_days` log files. Filenames sort
    /// lexicographically in date order, so newest-last sorting is enough — no
    /// need to stat or parse timestamps.
    fn prune(&self) {
        let mut files = match fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| is_event_log(path))
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        if files.len() <= self.retain_days {
            return;
        }
        files.sort();
        let stale = files.len() - self.retain_days;
        for path in files.into_iter().take(stale) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Whether `path` is one of our own rotated log files, so pruning can never
/// delete something else that happens to share the directory.
fn is_event_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("events-") && name.ends_with(".jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(date: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&format!("{date}T12:00:00Z")).unwrap().with_timezone(&Utc)
    }

    fn record(msg: &str) -> serde_json::Value {
        serde_json::json!({ "message": msg })
    }

    fn lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path).unwrap().lines().map(str::to_string).collect()
    }

    #[test]
    fn appends_one_json_line_per_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = EventLog::new(dir.path(), 7);

        assert!(log.append(&record("first"), at("2026-07-18")));
        assert!(log.append(&record("second"), at("2026-07-18")));

        let written = lines(&dir.path().join("events-2026-07-18.jsonl"));
        assert_eq!(written.len(), 2);
        let parsed: serde_json::Value = serde_json::from_str(&written[0]).unwrap();
        assert_eq!(parsed["message"], "first");
    }

    #[test]
    fn creates_the_directory_lazily_on_first_write() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("does").join("not").join("exist");
        let mut log = EventLog::new(&nested, 7);

        assert!(!nested.exists(), "constructing a log must not touch the filesystem");
        assert!(log.append(&record("hello"), at("2026-07-18")));
        assert!(nested.join("events-2026-07-18.jsonl").exists());
    }

    #[test]
    fn rolls_over_to_a_new_file_when_the_date_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = EventLog::new(dir.path(), 7);

        log.append(&record("monday"), at("2026-07-18"));
        log.append(&record("tuesday"), at("2026-07-19"));

        assert_eq!(lines(&dir.path().join("events-2026-07-18.jsonl")).len(), 1);
        assert_eq!(lines(&dir.path().join("events-2026-07-19.jsonl")).len(), 1);
    }

    #[test]
    fn reopens_an_existing_day_without_truncating_it() {
        let dir = tempfile::tempdir().unwrap();

        let mut first = EventLog::new(dir.path(), 7);
        first.append(&record("before restart"), at("2026-07-18"));
        drop(first);

        let mut second = EventLog::new(dir.path(), 7);
        second.append(&record("after restart"), at("2026-07-18"));

        let written = lines(&dir.path().join("events-2026-07-18.jsonl"));
        assert_eq!(written.len(), 2, "a restart must append, not truncate");
    }

    #[test]
    fn prunes_files_beyond_the_retention_window() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = EventLog::new(dir.path(), 2);

        for day in ["2026-07-15", "2026-07-16", "2026-07-17", "2026-07-18"] {
            log.append(&record(day), at(day));
        }

        // Pruning runs on rollover, so after opening the 18th only it and the
        // 17th survive.
        assert!(!dir.path().join("events-2026-07-15.jsonl").exists());
        assert!(!dir.path().join("events-2026-07-16.jsonl").exists());
        assert!(dir.path().join("events-2026-07-17.jsonl").exists());
        assert!(dir.path().join("events-2026-07-18.jsonl").exists());
    }

    #[test]
    fn pruning_leaves_unrelated_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        let bystander = dir.path().join("important.txt");
        fs::write(&bystander, "not ours").unwrap();

        let mut log = EventLog::new(dir.path(), 1);
        log.append(&record("a"), at("2026-07-17"));
        log.append(&record("b"), at("2026-07-18"));

        assert!(bystander.exists(), "pruning must only delete our own rotated logs");
    }
}
