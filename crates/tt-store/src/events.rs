//! Calendar-event storage: the RFC-3339-preserving write path, the day-window
//! helpers, and the next-meeting read used by the focus features.

use rusqlite::params;

use crate::model::*;
use crate::{Result, Store};

/// Local midnight of `date` as epoch ms, resolving DST edges rather than
/// giving up on them: an ambiguous midnight takes the earlier instant, and a
/// nonexistent one (spring-forward at 00:00) walks forward to the first valid
/// minute of that day. `None` only if the whole day is unrepresentable.
fn local_midnight(date: chrono::NaiveDate) -> Option<i64> {
    use chrono::{Local, LocalResult, TimeZone};

    match date.and_hms_opt(0, 0, 0).map(|dt| Local.from_local_datetime(&dt)) {
        Some(LocalResult::Single(dt)) => return Some(dt.timestamp_millis()),
        // Fall-back fold: two instants map to this local time. Take the earlier
        // so the day window still starts at the first occurrence of midnight.
        Some(LocalResult::Ambiguous(earlier, _)) => return Some(earlier.timestamp_millis()),
        _ => {}
    }
    // Spring-forward at 00:00: midnight doesn't exist. Step forward a minute at
    // a time to the first instant that does — bounded, since a DST jump is
    // never more than a couple of hours.
    for minute in 1..=180 {
        if let Some(dt) = date.and_hms_opt(0, 0, 0).map(|dt| dt + chrono::Duration::minutes(minute))
            && let Some(resolved) = Local.from_local_datetime(&dt).earliest()
        {
            return Some(resolved.timestamp_millis());
        }
    }
    None
}

impl Store {
    // --- Writes -----------------------------------------------------------

    /// The `[start, end)` epoch-ms bounds of the local calendar day containing
    /// `reference_ms` — the window callers pass to
    /// [`Store::replace_events_for_source`].
    ///
    /// This lives here, beside the delete it scopes, because **every writer
    /// must agree on it**. It previously existed twice — once in the collector,
    /// once in the MCP tool — with different DST fallbacks: one widened to a
    /// ±1-day window, the other collapsed to an empty one. Both fed the same
    /// scoped `DELETE`, so on a DST-transition day the same calendar day would
    /// sweep two days of rows when written by the collector and none when
    /// written over MCP. One destructive window, one implementation.
    ///
    /// DST is handled rather than punted on:
    /// - An **ambiguous** local midnight (a fall-back fold, real in zones like
    ///   Brazil, Chile and Cuba that transition at midnight) resolves to the
    ///   *earlier* instant, so the window still covers the whole civil day. The
    ///   old code used `.single()` here, which returned `None` for this case and
    ///   silently skipped the delete twice a year.
    /// - A **nonexistent** local midnight (spring-forward at 00:00) walks
    ///   forward to the first valid instant of that day.
    /// - Only if both boundaries are unresolvable does it fall back — to the
    ///   empty window, never a wider one. Deleting nothing leaves stale rows a
    ///   later pull fixes; deleting too much destroys data no pull restores.
    pub fn local_day_bounds(reference_ms: i64) -> (i64, i64) {
        use chrono::{Duration, Local, TimeZone};

        let Some(reference) = Local.timestamp_millis_opt(reference_ms).single() else {
            return (reference_ms, reference_ms);
        };
        let date = reference.date_naive();
        let start = local_midnight(date);
        let end = local_midnight(date + Duration::days(1));
        match (start, end) {
            (Some(start), Some(end)) => (start, end),
            _ => (reference_ms, reference_ms),
        }
    }

    /// Drop calendar events older than the retention window, independent of any
    /// write.
    ///
    /// [`Store::replace_events_for_source`] sweeps as a side effect, which is
    /// enough while some calendar is still being pulled — but not when the last
    /// one is switched off. Then no write ever happens again, the sweep never
    /// runs, and whatever was in the table stays forever: `calendar_next` keeps
    /// returning a meeting from the day the user turned collection off, with an
    /// ever-more-negative `minutesUntil` feeding the countdown and the
    /// meeting-start notification. The collector calls this even on the
    /// nothing-to-do path for exactly that reason.
    ///
    /// Returns how many rows were removed.
    pub fn sweep_old_events(&self, now_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM events WHERE starts_at_utc < ?1",
            params![utc_key(now_ms.saturating_sub(EVENT_RETAIN_MS))],
        )?)
    }

    /// Replace one calendar's events within one day window, leaving every other
    /// calendar — and every other day — untouched.
    ///
    /// This is deliberately *not* a full-table swap. Several calendars (personal
    /// Google, work Outlook) are pulled independently and merged into a single
    /// timeline; a global `DELETE FROM events` meant whichever pulled second
    /// erased the first. Scoping the delete to `(source, day)` makes each pull
    /// idempotent within its own lane.
    ///
    /// `source` is assigned by the *caller*, never by the data: it identifies
    /// which configured calendar this pull represents, and [`EventInput`]
    /// therefore has no `source` field — a model-authored payload must not be
    /// able to name the lane it writes into.
    ///
    /// The window is `[day_start_ms, day_end_ms)` against `start_ts`, passed in
    /// rather than derived here so the local-day boundary (and DST) stays the
    /// caller's decision and tests stay deterministic. Events outside it are
    /// inserted but will not be swept by this call — pass a window that actually
    /// contains them.
    pub fn replace_events_for_source(
        &self,
        source: &str,
        day_start_ms: i64,
        day_end_ms: i64,
        events: &[EventInput],
        now_ms: i64,
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM events WHERE source = ?1
               AND starts_at_utc >= ?2 AND starts_at_utc < ?3",
            params![source, utc_key(day_start_ms), utc_key(day_end_ms)],
        )?;
        // Retention. The delete above is scoped to one lane and one day, so
        // unlike the full-table swap it replaced it bounds nothing over time:
        // yesterday's meetings, and every row belonging to a calendar the user
        // has since renamed or removed, would otherwise accumulate forever.
        // Sweeping by age (not by source) is what catches the orphaned-lane
        // case, since no per-source write will ever visit those rows again.
        // Cheap to run here — this path fires per collector tick, not per read.
        // Delegated to `sweep_old_events` rather than repeating its SQL, so
        // write-time and standalone sweeping cannot drift apart; `tx` is an
        // `unchecked_transaction` on `self.conn`, so the call joins it.
        self.sweep_old_events(now_ms)?;
        // De-duplicate by external_id before inserting. The upsert below would
        // otherwise let a repeated id overwrite its own earlier row inside this
        // loop — one row lands, the other vanishes, and the returned count still
        // claims both were written. A model emitting the same recurring-meeting
        // instance twice is exactly how that happens, so collapse it here and
        // report what actually landed. Last occurrence wins, matching the
        // upsert's own semantics.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let deduped: Vec<&EventInput> = events
            .iter()
            .rev()
            .filter(|e| seen.insert(e.external_id.as_str()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO events
                   (source, external_id, title, starts_at, ends_at, attendees, location, join_url,
                    updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(source, external_id) DO UPDATE SET
                   title = excluded.title,
                   starts_at = excluded.starts_at,
                   ends_at = excluded.ends_at,
                   attendees = excluded.attendees,
                   location = excluded.location,
                   join_url = excluded.join_url,
                   updated_at = excluded.updated_at",
            )?;
            for e in &deduped {
                stmt.execute(params![
                    source,
                    e.external_id,
                    e.title,
                    e.start.to_rfc3339(),
                    e.end.map(|end| end.to_rfc3339()),
                    serde_json::to_string(&e.attendees)?,
                    e.location,
                    e.join_url,
                    now_ms,
                ])?;
            }
        }
        tx.commit()?;
        Ok(deduped.len())
    }

    // --- Queries ----------------------------------------------------------

    /// Events starting within `[start_ms, end_ms)`, ordered by start time.
    pub fn events_between(&self, start_ms: i64, end_ms: i64) -> Result<Vec<CalEvent>> {
        self.query_events(
            &format!(
                "SELECT {EVENT_COLS} FROM events
                 WHERE starts_at_utc >= ?1 AND starts_at_utc < ?2 ORDER BY starts_at_utc ASC"
            ),
            params![utc_key(start_ms), utc_key(end_ms)],
        )
    }

    /// The meeting to surface at `now_ms`: the one in progress right now, or
    /// the soonest still to start — whichever begins first.
    ///
    /// An event counts as in progress while `start_ts <= now_ms < end_ts`, so
    /// a meeting stays selected until it actually ends rather than vanishing
    /// the instant it starts. An event with no `end_ts` is treated as a point
    /// in time and is only returned while still in the future
    /// (`start_ts >= now_ms`). Returns `None` once the last event has ended.
    pub fn current_or_next_event(&self, now_ms: i64) -> Result<Option<CalEvent>> {
        Ok(self
            .query_events(
                &format!(
                    "SELECT {EVENT_COLS} FROM events
                     WHERE (ends_at_utc IS NOT NULL AND ends_at_utc > ?1)
                        OR (ends_at_utc IS NULL AND starts_at_utc >= ?1)
                     ORDER BY starts_at_utc ASC LIMIT 1"
                ),
                [utc_key(now_ms)],
            )?
            .into_iter()
            .next())
    }

    pub(crate) fn query_events(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<CalEvent>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                source,
                external_id,
                title,
                starts_at,
                ends_at,
                attendees_json,
                location,
                join_url,
            ) = row?;
            let attendees: Vec<String> = serde_json::from_str(&attendees_json)?;
            // Rows are written from a `DateTime`, so a value that no longer
            // parses means the column was edited by hand or by another tool.
            // Skip that row rather than failing the whole query: one bad row
            // must not blank the countdown, and it ages out with retention.
            let Some(start) = parse_rfc3339(&starts_at) else {
                log_unparseable_event(&external_id, &starts_at);
                continue;
            };
            out.push(CalEvent {
                id,
                source,
                external_id,
                title,
                start,
                end: ends_at.as_deref().and_then(parse_rfc3339),
                attendees,
                location,
                join_url,
            });
        }
        Ok(out)
    }
}
