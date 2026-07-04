//! Versioned `claude -p` prompt strings for the data-hub collectors.
//!
//! Each prompt demands a bare JSON payload (no prose, no code fences) so the
//! lenient extractor in [`crate::extract_json`] has the best chance of a clean
//! parse. Bump the `v1`/`v2` markers when a prompt's contract changes.

/// Calendar collector (v1): today's + next-7-days events from connected
/// calendar tools, as a JSON array.
pub const CALENDAR: &str = "\
Using your connected calendar tools, list my events for today and the next 7 \
days. Respond with ONLY a JSON array, no prose, no code fences. Each element: \
{\"externalId\": string (stable event id), \"title\": string, \"startTs\": \
integer (epoch milliseconds), \"endTs\": integer (epoch milliseconds), \
\"attendees\": array of attendee display-name strings, \"location\": string, \
\"joinUrl\": string}. Use integer epoch-millisecond timestamps. Omit any field \
whose value is null or unknown. If there are no events, respond with [].";

/// Email + tasks collector (v1): triaged inbox plus extracted action items,
/// as a single JSON object with `emails` and `tasks` arrays.
pub const EMAIL: &str = "\
Using your connected email tools, review my ~25 most recent actionable inbox \
emails and also extract any action items they imply. Respond with ONLY a JSON \
object, no prose, no code fences, of the shape {\"emails\": [...], \"tasks\": \
[...]}. Each emails element: {\"externalId\": string (stable message id), \
\"fromName\": string, \"fromAddr\": string, \"subject\": string, \"summary\": \
string (one line), \"tag\": one of \"needs_reply\", \"invite\", \"fyi\", \
\"receivedTs\": integer (epoch milliseconds)}. Each tasks element: {\"text\": \
string, \"dueTs\": integer (epoch milliseconds) or omitted, \"sourceRef\": \
string (the related message id) or omitted}. If there is nothing to report, \
respond with {\"emails\": [], \"tasks\": []}.";
