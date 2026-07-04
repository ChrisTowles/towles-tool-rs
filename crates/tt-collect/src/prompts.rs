//! Built-in `claude -p` prompt strings for the calendar collector.
//!
//! Calendar is reduced to a single purpose — *how long until my next meeting* —
//! so each prompt asks only for **today's** events. The provider variants differ
//! only in which MCP they drive (Google at home, Outlook at work); the JSON
//! contract is identical so [`crate::extract_json`] and [`tt_store::EventInput`]
//! stay the same. Bump the `v1` marker when a prompt's contract changes.

/// Calendar collector (v1) — Google Calendar via MCP, today only.
pub const CALENDAR_GOOGLE: &str = "\
Using the Google Calendar MCP, list the events on my primary calendar for today \
only, in my local timezone. Respond with ONLY a JSON array, no prose, no code \
fences. Each element: {\"externalId\": string (stable event id), \"title\": \
string, \"startTs\": integer (epoch milliseconds), \"endTs\": integer (epoch \
milliseconds), \"attendees\": array of attendee display-name strings, \
\"location\": string, \"joinUrl\": string}. Skip all-day events and events I \
have declined. Omit any field whose value is null or unknown. If there are no \
events, respond with [].";

/// Calendar collector (v1) — Outlook / Microsoft 365 via MCP, today only.
pub const CALENDAR_OUTLOOK: &str = "\
Using the Outlook (Microsoft 365) MCP, list the events on my default calendar \
for today only, in my local timezone. Respond with ONLY a JSON array, no prose, \
no code fences. Each element: {\"externalId\": string (stable event id), \
\"title\": string, \"startTs\": integer (epoch milliseconds), \"endTs\": \
integer (epoch milliseconds), \"attendees\": array of attendee display-name \
strings, \"location\": string, \"joinUrl\": string}. Skip all-day events and \
events I have declined. Omit any field whose value is null or unknown. If there \
are no events, respond with [].";
