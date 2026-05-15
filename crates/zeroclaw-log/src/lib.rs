//! Unified log emission surface for the ZeroClaw workspace.
//!
//! Every crate that emits domain events (agent activity, channel I/O, cron
//! runs, tool calls, memory ops, session lifecycle, errors) goes through
//! [`record!`]. That single emission point fans out to:
//!
//! 1. A `tracing::event!` at the matching severity so `RUST_LOG`-gated
//!    terminal output and any external `tracing-subscriber` consumer see
//!    the event with structured `key=value` fields.
//! 2. The persisted JSONL log at `<workspace>/state/runtime-trace.jsonl`
//!    (when `[observability] log_persistence` is `"rolling"` or `"full"`).
//! 3. The process-wide broadcast channel so the dashboard's SSE stream
//!    sees every event live.
//!
//! Schema is an OTel/ECS hybrid with a ZeroClaw-domain `zeroclaw.*`
//! namespace for the alias-bound attribution fields. See [`event::LogEvent`].

pub mod broadcast;
pub mod chain;
pub mod config;
pub mod event;
pub mod migrate;
pub mod reader;
pub mod tool_io;
pub mod writer;

#[doc(hidden)]
pub use chrono;
#[doc(hidden)]
pub use serde_json;
#[doc(hidden)]
pub use tracing;
#[doc(hidden)]
pub use uuid;

pub use broadcast::{
    LogBroadcastSender, clear_broadcast_hook, current_broadcast_hook, set_broadcast_hook, subscribe,
};
pub use chain::display_chain;
pub use config::{ResolvedPolicy, StoragePolicy, ToolIoPolicy};
pub use event::{
    EventCategory, EventOutcome, LogEvent, Severity, severity_text_from_number,
    severity_text_from_tracing_level,
};
pub use migrate::migrate_legacy_jsonl_in_place;
pub use reader::{LogFilter, LogPage, current_log_path, find_event_by_id, load_page};
pub use tool_io::{ToolIoCapture, capture_tool_input, capture_tool_output};
pub use writer::{init_from_config, record_event, runtime_trace_path};

mod r#macro;
