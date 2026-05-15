//! Paginated stream reader for the JSONL log file.
//!
//! RAM contract: at any moment, in-memory state is bounded by `limit`
//! (the number of events the caller asked for) plus a single-line read
//! buffer. We do NOT slurp the whole file into a `String`.
//!
//! The pagination model is cursor-by-timestamp + cursor-by-id. Callers
//! pass `until_ts` to ask for "events strictly older than this timestamp
//! (or older with the same timestamp by id ordering)". Returning page
//! includes `next_cursor` which is the oldest event's `(timestamp, id)`
//! pair — callers use that to ask for the next page.
//!
//! Filters apply lazily: the reader scans backwards from EOF, decoding
//! each line, applying the filter predicate, and stopping when it has
//! collected `limit` matches or exhausted the file. Worst case for tight
//! filters: the whole file is scanned. Best case (no filter): only
//! `limit` lines decoded.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::event::LogEvent;

/// Filter parameters for [`load_page`]. Each field is independent; an
/// event must match ALL provided constraints to be included.
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    /// RFC 3339 lower bound (inclusive).
    pub since_ts: Option<String>,
    /// RFC 3339 upper bound (exclusive — used by pagination cursor).
    pub until_ts: Option<String>,
    /// Match against the cursor's id when `until_ts` ties.
    pub until_id: Option<String>,
    /// Match exact event.action (case-insensitive).
    pub action: Option<String>,
    /// Match exact event.category (case-insensitive).
    pub category: Option<String>,
    /// Match exact event.outcome (case-insensitive).
    pub outcome: Option<String>,
    /// Minimum severity_number.
    pub severity_min: Option<u8>,
    /// Match exact zeroclaw.agent_alias.
    pub agent: Option<String>,
    /// Match exact zeroclaw.channel (composite `<type>.<alias>`).
    pub channel: Option<String>,
    /// Match exact zeroclaw.channel_type.
    pub channel_type: Option<String>,
    /// Match exact zeroclaw.tool.
    pub tool: Option<String>,
    /// Match exact trace_id.
    pub trace_id: Option<String>,
    /// Substring search across message + attributes.
    pub q: Option<String>,
    /// Hide events with event.category == "internal" by default.
    pub hide_internal: bool,
}

/// One page returned by [`load_page`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogPage {
    pub events: Vec<LogEvent>,
    /// `Some((timestamp, id))` when more older events may exist; pass to
    /// the next call as `(until_ts, until_id)`.
    pub next_cursor: Option<(String, String)>,
    /// True when the file was fully scanned. UI uses this to disable
    /// "load older" affordances.
    pub at_end: bool,
}

/// Load one page of events. Newest first.
///
/// Implementation: we open the file, read it line by line into a fixed
/// in-memory buffer (capped at `limit` matched events). To preserve the
/// "newest first" order without reading from the tail, we accumulate
/// matched events into a `VecDeque`, keeping the cap = `limit`, popping
/// the front when overflowed. Final result is reversed in place. That
/// gives us a one-pass, single-allocation-bounded reader without needing
/// `mmap` or reverse-byte-stream gymnastics.
pub fn load_page(path: &Path, filter: &LogFilter, limit: usize) -> Result<LogPage> {
    let limit = limit.clamp(1, 10_000);

    if !path.exists() {
        return Ok(LogPage {
            events: Vec::new(),
            next_cursor: None,
            at_end: true,
        });
    }

    let file = File::open(path).with_context(|| format!("opening log: {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut window: VecDeque<LogEvent> = VecDeque::with_capacity(limit + 1);
    let needle = filter.q.as_deref().map(|s| s.to_ascii_lowercase());
    // `dropped_older` records whether we ever pushed past `limit` and
    // had to evict the oldest matching event. If false at the end, every
    // matching event in the file is in `window` — meaning there are no
    // older results the caller could page back to.
    let mut dropped_older = false;

    for line in reader.lines() {
        let line = line.context("reading log line")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let event: LogEvent = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(err) => {
                tracing::trace!(
                    target: "zeroclaw_log",
                    error = ?err,
                    "log: skipping malformed JSONL line"
                );
                continue;
            }
        };

        if !matches_filter(&event, filter, needle.as_deref()) {
            continue;
        }

        window.push_back(event);
        if window.len() > limit {
            window.pop_front();
            dropped_older = true;
        }
    }

    let mut events: Vec<LogEvent> = window.into_iter().collect();
    // Reverse so newest is first.
    events.reverse();

    // next_cursor is the OLDEST event in the page (the last one in
    // newest-first ordering = events.last()). Caller uses it as
    // `until_ts` / `until_id` for the next "load older" request.
    let next_cursor = events.last().map(|e| (e.timestamp.clone(), e.id.clone()));

    // We've reached the tail of the matched set when no older matching
    // events were ever discarded during the scan.
    let at_end = !dropped_older;

    Ok(LogPage {
        events,
        next_cursor,
        at_end,
    })
}

fn matches_filter(event: &LogEvent, filter: &LogFilter, needle: Option<&str>) -> bool {
    if filter.hide_internal && event.event.category == "internal" {
        return false;
    }
    if let Some(ref since) = filter.since_ts
        && event.timestamp.as_str() < since.as_str()
    {
        return false;
    }
    if let Some(ref until) = filter.until_ts {
        // Cursor pagination: include events strictly older than the
        // cursor. If the timestamps tie, fall back to id ordering for
        // deterministic pagination.
        match event.timestamp.as_str().cmp(until.as_str()) {
            std::cmp::Ordering::Greater => return false,
            std::cmp::Ordering::Equal => {
                if let Some(ref until_id) = filter.until_id
                    && event.id.as_str() >= until_id.as_str()
                {
                    return false;
                }
            }
            std::cmp::Ordering::Less => {}
        }
    }
    if let Some(ref action) = filter.action
        && !event.event.action.eq_ignore_ascii_case(action)
    {
        return false;
    }
    if let Some(ref category) = filter.category
        && !event.event.category.eq_ignore_ascii_case(category)
    {
        return false;
    }
    if let Some(ref outcome) = filter.outcome
        && !event.event.outcome.eq_ignore_ascii_case(outcome)
    {
        return false;
    }
    if let Some(min) = filter.severity_min
        && event.severity_number < min
    {
        return false;
    }
    if let Some(ref agent) = filter.agent
        && event.zeroclaw.agent_alias.as_deref() != Some(agent.as_str())
    {
        return false;
    }
    if let Some(ref channel) = filter.channel
        && event.zeroclaw.channel.as_deref() != Some(channel.as_str())
    {
        return false;
    }
    if let Some(ref ty) = filter.channel_type
        && event.zeroclaw.channel_type.as_deref() != Some(ty.as_str())
    {
        return false;
    }
    if let Some(ref tool) = filter.tool
        && event.zeroclaw.tool.as_deref() != Some(tool.as_str())
    {
        return false;
    }
    if let Some(ref tid) = filter.trace_id
        && event.trace_id.as_deref() != Some(tid.as_str())
    {
        return false;
    }
    if let Some(n) = needle {
        let hay_msg = event.message.as_deref().unwrap_or("").to_ascii_lowercase();
        let hay_attrs = event.attributes.to_string().to_ascii_lowercase();
        if !hay_msg.contains(n) && !hay_attrs.contains(n) {
            return false;
        }
    }
    true
}

/// Find a single event by id. Scans the file backwards from the end.
pub fn find_event_by_id(path: &Path, id: &str) -> Result<Option<LogEvent>> {
    if !path.exists() {
        return Ok(None);
    }
    let file = File::open(path).with_context(|| format!("opening log: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut found: Option<LogEvent> = None;
    for line in reader.lines() {
        let line = line.context("reading log line")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<LogEvent>(trimmed)
            && event.id == id
        {
            found = Some(event); // Don't break — last write wins for duplicate ids.
        }
    }
    Ok(found)
}

/// Helper for the gateway: the path the writer is configured to use.
#[must_use]
pub fn current_log_path() -> Option<PathBuf> {
    crate::writer::runtime_trace_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventCategory, Severity};
    use std::io::Write;

    fn write_jsonl(path: &Path, events: &[LogEvent]) {
        let mut f = std::fs::File::create(path).unwrap();
        for e in events {
            let line = serde_json::to_string(e).unwrap();
            f.write_all(line.as_bytes()).unwrap();
            f.write_all(b"\n").unwrap();
        }
    }

    fn make_event(action: &str, agent: Option<&str>) -> LogEvent {
        let mut e = LogEvent::new(Severity::Info, action, EventCategory::Agent);
        e.zeroclaw.agent_alias = agent.map(String::from);
        e
    }

    #[test]
    fn empty_file_returns_at_end() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let page = load_page(&path, &LogFilter::default(), 10).unwrap();
        assert!(page.events.is_empty());
        assert!(page.at_end);
    }

    #[test]
    fn returns_newest_first_within_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let mut events = Vec::new();
        for i in 0..5 {
            let mut e = make_event("test", None);
            // Force monotonically increasing timestamp.
            e.timestamp = format!("2026-05-15T19:00:0{i}.000Z");
            e.message = Some(format!("event-{i}"));
            events.push(e);
        }
        write_jsonl(&path, &events);

        let page = load_page(&path, &LogFilter::default(), 3).unwrap();
        assert_eq!(page.events.len(), 3);
        assert_eq!(page.events[0].message.as_deref(), Some("event-4"));
        assert_eq!(page.events[1].message.as_deref(), Some("event-3"));
        assert_eq!(page.events[2].message.as_deref(), Some("event-2"));
        assert!(!page.at_end);
    }

    #[test]
    fn filter_by_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let events = vec![
            make_event("a", Some("clamps")),
            make_event("b", Some("glados")),
            make_event("c", Some("clamps")),
        ];
        write_jsonl(&path, &events);

        let filter = LogFilter {
            agent: Some("clamps".into()),
            ..Default::default()
        };
        let page = load_page(&path, &filter, 10).unwrap();
        assert_eq!(page.events.len(), 2);
    }

    #[test]
    fn hide_internal_drops_internal_category() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let mut e1 = make_event("a", None);
        e1.event.category = "agent".into();
        let mut e2 = make_event("b", None);
        e2.event.category = "internal".into();
        write_jsonl(&path, &[e1, e2]);

        let filter = LogFilter {
            hide_internal: true,
            ..Default::default()
        };
        let page = load_page(&path, &filter, 10).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].event.action, "a");
    }

    #[test]
    fn substring_query_matches_message_and_attributes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let mut e1 = make_event("a", None);
        e1.message = Some("alpha bravo".into());
        let mut e2 = make_event("b", None);
        e2.attributes = serde_json::json!({ "k": "delta echo" });
        let mut e3 = make_event("c", None);
        e3.message = Some("foxtrot".into());
        write_jsonl(&path, &[e1, e2, e3]);

        let filter = LogFilter {
            q: Some("bravo".into()),
            ..Default::default()
        };
        let page = load_page(&path, &filter, 10).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].event.action, "a");

        let filter2 = LogFilter {
            q: Some("delta".into()),
            ..Default::default()
        };
        let page2 = load_page(&path, &filter2, 10).unwrap();
        assert_eq!(page2.events.len(), 1);
        assert_eq!(page2.events[0].event.action, "b");
    }

    #[test]
    fn cursor_pagination_returns_older_pages() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("trace.jsonl");
        let mut events = Vec::new();
        for i in 0..6 {
            let mut e = make_event("test", None);
            e.timestamp = format!("2026-05-15T19:00:0{i}.000Z");
            e.message = Some(format!("event-{i}"));
            events.push(e);
        }
        write_jsonl(&path, &events);

        let page1 = load_page(&path, &LogFilter::default(), 3).unwrap();
        assert_eq!(page1.events[0].message.as_deref(), Some("event-5"));
        let (cur_ts, cur_id) = page1.next_cursor.unwrap();

        let filter = LogFilter {
            until_ts: Some(cur_ts),
            until_id: Some(cur_id),
            ..Default::default()
        };
        let page2 = load_page(&path, &filter, 3).unwrap();
        assert_eq!(page2.events[0].message.as_deref(), Some("event-2"));
        assert_eq!(page2.events[1].message.as_deref(), Some("event-1"));
        assert_eq!(page2.events[2].message.as_deref(), Some("event-0"));
        assert!(page2.at_end);
    }
}
