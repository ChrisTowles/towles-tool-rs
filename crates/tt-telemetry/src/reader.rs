use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use crate::event_log::event_log_date;
use crate::schema::{
    FIELD_DURATION_MS, FIELD_KIND, FIELD_LEVEL, FIELD_NAME, FIELD_TARGET, FIELD_TS, FIELD_TT_TASK,
};
use crate::{Error, Result, TelemetryRecord};

/// Dates with a log file in `dir` (`events-<date>.jsonl`), newest first.
/// Filenames sort lexicographically in date order; recognizing the filename
/// is [`event_log_date`]'s job so pruning and this listing can't disagree
/// about what counts as a log file.
pub fn list_days(dir: &Path) -> Result<Vec<String>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut days: Vec<String> = fs::read_dir(dir)?
        .flatten()
        .filter_map(|entry| Some(event_log_date(&entry.path())?.to_string()))
        .collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    Ok(days)
}

/// All records in `dir`'s log file for `date`, in the order they were
/// written. A missing file (no telemetry that day) is an empty list, not an
/// error. Lines that fail to parse or lack the base fields every record
/// carries are skipped — a single malformed line must not hide the rest of
/// the day.
pub fn read_day(dir: &Path, date: &str) -> Result<Vec<TelemetryRecord>> {
    let path = dir.join(format!("events-{date}.jsonl"));
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Io(e)),
    };
    Ok(content.lines().filter_map(parse_line).collect())
}

fn take_string(obj: &mut Map<String, Value>, key: &str) -> Option<String> {
    match obj.remove(key)? {
        Value::String(s) => Some(s),
        _ => None,
    }
}

fn parse_line(line: &str) -> Option<TelemetryRecord> {
    let value: Value = serde_json::from_str(line).ok()?;
    let Value::Object(mut obj) = value else {
        return None;
    };

    let ts = take_string(&mut obj, FIELD_TS)?;
    let kind = take_string(&mut obj, FIELD_KIND)?;
    let level = take_string(&mut obj, FIELD_LEVEL)?;
    let target = take_string(&mut obj, FIELD_TARGET)?;
    let name = take_string(&mut obj, FIELD_NAME)?;
    let tt_task = take_string(&mut obj, FIELD_TT_TASK);
    let duration_ms = obj.remove(FIELD_DURATION_MS).and_then(|v| v.as_i64());
    for key in crate::schema::RESOURCE_KEYS {
        obj.remove(*key);
    }

    Some(TelemetryRecord {
        ts,
        kind,
        level,
        target,
        name,
        tt_task,
        duration_ms,
        fields: Value::Object(obj),
        raw: line.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_day(dir: &Path, date: &str, lines: &[&str]) {
        fs::write(dir.join(format!("events-{date}.jsonl")), lines.join("\n")).unwrap();
    }

    /// Writes a real event through [`crate::EventLogLayer`] (the same path
    /// `init` installs) and reads it back through [`read_day`] — the
    /// round-trip the crate's schema-sharing is actually for, so a field
    /// rename on either side fails a test instead of silently dropping the
    /// record.
    #[test]
    fn round_trips_a_span_through_the_real_writer() {
        use chrono::{DateTime, Utc};
        use tracing_subscriber::prelude::*;

        use crate::event_log::EventLog;
        use crate::layer::EventLogLayer;

        fn fixed_now() -> DateTime<Utc> {
            DateTime::parse_from_rfc3339("2026-07-22T12:00:00Z").unwrap().with_timezone(&Utc)
        }

        let dir = tempfile::tempdir().unwrap();
        let mut resource = serde_json::Map::new();
        resource.insert("tt.task".into(), Value::from("feat-x"));
        let layer =
            EventLogLayer::new(EventLog::new(dir.path(), 7), resource).with_clock(fixed_now);

        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            tracing::info_span!("process.spawn", cmd = "gh").in_scope(|| {});
        });

        let records = read_day(dir.path(), "2026-07-22").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, "span");
        assert_eq!(records[0].name, "process.spawn");
        assert_eq!(records[0].tt_task.as_deref(), Some("feat-x"));
        assert!(records[0].duration_ms.is_some());
        assert_eq!(records[0].fields["cmd"], "gh");
    }

    #[test]
    fn list_days_returns_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        write_day(dir.path(), "2026-07-20", &["{}"]);
        write_day(dir.path(), "2026-07-22", &["{}"]);
        write_day(dir.path(), "2026-07-21", &["{}"]);

        assert_eq!(list_days(dir.path()).unwrap(), vec!["2026-07-22", "2026-07-21", "2026-07-20"]);
    }

    #[test]
    fn list_days_on_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(list_days(&dir.path().join("nope")).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn read_day_on_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_day(dir.path(), "2026-07-22").unwrap().is_empty());
    }

    #[test]
    fn parses_an_event_and_strips_resource_fields_into_the_record() {
        let dir = tempfile::tempdir().unwrap();
        write_day(
            dir.path(),
            "2026-07-22",
            &[
                r#"{"service.name":"tt-app","process.pid":1,"tt.task":"feat-x","ts":"2026-07-22T00:00:00+00:00","kind":"event","level":"INFO","target":"tt_app_lib","name":"ui.action","action":"repo.icon_set"}"#,
            ],
        );

        let records = read_day(dir.path(), "2026-07-22").unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.kind, "event");
        assert_eq!(r.level, "INFO");
        assert_eq!(r.target, "tt_app_lib");
        assert_eq!(r.name, "ui.action");
        assert_eq!(r.tt_task.as_deref(), Some("feat-x"));
        assert_eq!(r.duration_ms, None);
        assert_eq!(r.fields["action"], "repo.icon_set");
        assert!(r.fields.get("service.name").is_none());
    }

    #[test]
    fn parses_a_span_with_duration() {
        let dir = tempfile::tempdir().unwrap();
        write_day(
            dir.path(),
            "2026-07-22",
            &[
                r#"{"ts":"2026-07-22T00:00:00+00:00","kind":"span","level":"INFO","target":"tt_exec","name":"process.spawn","duration_ms":842,"cmd":"gh"}"#,
            ],
        );

        let records = read_day(dir.path(), "2026-07-22").unwrap();
        assert_eq!(records[0].duration_ms, Some(842));
        assert_eq!(records[0].fields["cmd"], "gh");
    }

    #[test]
    fn skips_malformed_lines_without_failing_the_whole_read() {
        let dir = tempfile::tempdir().unwrap();
        write_day(
            dir.path(),
            "2026-07-22",
            &[
                "not json",
                r#"{"ts":"2026-07-22T00:00:00+00:00","kind":"event","level":"INFO","target":"t","name":"n"}"#,
            ],
        );

        assert_eq!(read_day(dir.path(), "2026-07-22").unwrap().len(), 1);
    }
}
