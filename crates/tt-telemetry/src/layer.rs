//! The `tracing` → [`EventLog`] bridge.
//!
//! Spans are recorded on *close* rather than on entry, so a single record
//! carries both the inputs (the fields set at creation) and the outcome
//! (duration, plus anything the body recorded before returning). That is what
//! makes the log answerable after the fact: one line per subprocess, holding
//! the command, where it ran, how long it took, and how it exited.

use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::event_log::EventLog;
use crate::schema::{
    FIELD_DURATION_MS, FIELD_KIND, FIELD_LEVEL, FIELD_NAME, FIELD_TARGET, FIELD_TS,
};

/// Per-span state carried from creation to close.
struct SpanState {
    fields: Map<String, Value>,
    opened: Instant,
}

/// Collects `tracing` field values into a JSON object.
#[derive(Default)]
struct JsonVisitor(Map<String, Value>);

impl Visit for JsonVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }

    /// Fallback for everything else (`?value` / `%value` and the `message`
    /// field), which `tracing` only exposes as `Debug`.
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), Value::from(format!("{value:?}")));
    }
}

/// A `tracing` layer that writes every event and every closed span to an
/// [`EventLog`] as one JSON record per line.
pub struct EventLogLayer {
    log: Mutex<EventLog>,
    /// Resource attributes stamped onto every record (service name, pid, task
    /// scope) — the OpenTelemetry notion of "who produced this".
    resource: Map<String, Value>,
    /// Wall clock, injectable so tests can pin timestamps.
    now: fn() -> DateTime<Utc>,
}

impl EventLogLayer {
    /// Build a layer writing to `log`, stamping `resource` on every record.
    pub fn new(log: EventLog, resource: Map<String, Value>) -> Self {
        Self { log: Mutex::new(log), resource, now: Utc::now }
    }

    /// Replace the wall clock. Test-only seam, `pub(crate)` so `reader`'s
    /// round-trip test can pin it too.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, now: fn() -> DateTime<Utc>) -> Self {
        self.now = now;
        self
    }

    /// Start a record with the fields every line carries.
    ///
    /// `now` is read once by the caller and threaded through to [`Self::write`]
    /// as well, so a record's `ts` and the file it rotates into can never
    /// disagree across a midnight boundary.
    fn base(
        &self,
        kind: &str,
        level: &tracing::Level,
        target: &str,
        name: &str,
        now: DateTime<Utc>,
    ) -> Map<String, Value> {
        let mut record = self.resource.clone();
        record.insert(FIELD_TS.into(), Value::from(now.to_rfc3339()));
        record.insert(FIELD_KIND.into(), Value::from(kind));
        record.insert(FIELD_LEVEL.into(), Value::from(level.as_str()));
        record.insert(FIELD_TARGET.into(), Value::from(target));
        record.insert(FIELD_NAME.into(), Value::from(name));
        record
    }

    /// Write a record, ignoring a poisoned lock rather than panicking inside
    /// the instrumentation of whatever poisoned it.
    fn write(&self, record: Map<String, Value>, now: DateTime<Utc>) {
        let mut log: MutexGuard<EventLog> = match self.log.lock() {
            Ok(log) => log,
            Err(poisoned) => poisoned.into_inner(),
        };
        log.append(&Value::Object(record), now);
    }
}

impl<S> tracing_subscriber::Layer<S> for EventLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut visitor = JsonVisitor::default();
        attrs.record(&mut visitor);
        span.extensions_mut().insert(SpanState { fields: visitor.0, opened: Instant::now() });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut visitor = JsonVisitor::default();
        values.record(&mut visitor);
        if let Some(state) = span.extensions_mut().get_mut::<SpanState>() {
            state.fields.extend(visitor.0);
        }
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let now = (self.now)();
        let meta = event.metadata();
        let mut record = self.base("event", meta.level(), meta.target(), meta.name(), now);
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);
        record.extend(visitor.0);
        self.write(record, now);
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else { return };
        let Some(state) = span.extensions_mut().remove::<SpanState>() else {
            return;
        };
        let now = (self.now)();
        let meta = span.metadata();
        let mut record = self.base("span", meta.level(), meta.target(), meta.name(), now);
        record.insert(
            FIELD_DURATION_MS.into(),
            Value::from(state.opened.elapsed().as_millis() as u64),
        );
        record.extend(state.fields);
        self.write(record, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-18T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    /// Run `body` with an `EventLogLayer` installed, returning the records it
    /// wrote. Uses a local subscriber so tests never fight over the global one.
    fn capture(body: impl FnOnce()) -> Vec<Value> {
        let dir = tempfile::tempdir().unwrap();
        let mut resource = Map::new();
        resource.insert("service.name".into(), Value::from("tt-test"));

        let layer =
            EventLogLayer::new(EventLog::new(dir.path(), 7), resource).with_clock(fixed_now);
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), body);

        let path = dir.path().join("events-2026-07-18.jsonl");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn writes_an_event_with_its_fields_and_resource_attributes() {
        let records = capture(|| {
            tracing::info!(target: "tt_test", answer = 42, "hello");
        });

        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["kind"], "event");
        assert_eq!(records[0]["level"], "INFO");
        assert_eq!(records[0]["target"], "tt_test");
        assert_eq!(records[0]["answer"], 42);
        assert_eq!(records[0]["service.name"], "tt-test");
        assert_eq!(records[0]["ts"], "2026-07-18T12:00:00+00:00");
    }

    #[test]
    fn writes_a_span_once_on_close_with_a_duration() {
        let records = capture(|| {
            let span = tracing::info_span!("unit_of_work", cmd = "gh");
            span.in_scope(|| {});
        });

        let spans: Vec<_> = records.iter().filter(|r| r["kind"] == "span").collect();
        assert_eq!(spans.len(), 1, "a span must produce exactly one record, at close");
        assert_eq!(spans[0]["name"], "unit_of_work");
        assert_eq!(spans[0]["cmd"], "gh");
        assert!(spans[0]["duration_ms"].is_u64());
    }

    #[test]
    fn span_record_includes_fields_set_after_creation() {
        let records = capture(|| {
            let span = tracing::info_span!("call", cmd = "gh", exit_code = tracing::field::Empty);
            span.in_scope(|| {});
            span.record("exit_code", 0);
            drop(span);
        });

        let span = records.iter().find(|r| r["kind"] == "span").unwrap();
        assert_eq!(span["exit_code"], 0, "outcome fields must reach the closed-span record");
    }

    #[test]
    fn nested_spans_each_produce_their_own_record() {
        let records = capture(|| {
            tracing::info_span!("outer").in_scope(|| {
                tracing::info_span!("inner").in_scope(|| {});
            });
        });

        let names: Vec<_> =
            records.iter().filter(|r| r["kind"] == "span").map(|r| r["name"].clone()).collect();
        assert_eq!(names, vec![Value::from("inner"), Value::from("outer")]);
    }
}
