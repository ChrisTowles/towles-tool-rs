//! The on-disk record's field names, shared by the writer ([`crate::layer`])
//! and the reader ([`crate::reader`]) so the two can't drift apart by having
//! each independently retype the same string literals — the whole reason
//! this crate holds both halves. A rename here breaks both sides at compile
//! time instead of silently dropping a field on one side only.

/// The base fields every record — event or span — carries.
pub(crate) const FIELD_TS: &str = "ts";
pub(crate) const FIELD_KIND: &str = "kind";
pub(crate) const FIELD_LEVEL: &str = "level";
pub(crate) const FIELD_TARGET: &str = "target";
pub(crate) const FIELD_NAME: &str = "name";

/// Present only on `kind: "span"` records.
pub(crate) const FIELD_DURATION_MS: &str = "duration_ms";

/// Resource attributes [`crate::resource`] stamps on every record. `tt.task`
/// is read into its own typed field by the reader; the rest are metadata the
/// reader strips out of `fields` rather than surfacing.
pub(crate) const FIELD_TT_TASK: &str = "tt.task";
pub(crate) const RESOURCE_KEYS: &[&str] = &["service.name", "service.version", "process.pid"];
