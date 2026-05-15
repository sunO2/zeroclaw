//! Canonical event schema. OTel logs data model + ECS attribute
//! conventions, with a `zeroclaw.*` namespace for the alias-bound
//! domain attribution fields.
//!
//! On-disk JSON shape is the canonical contract — third-party tail
//! consumers parse `serde_json::Value` and walk the keys. This struct is
//! `pub(crate)` to keep external consumers off the typed surface.

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// OTel severity buckets. Stored alongside `severity_number` so consumers
/// can range-compare numerically and pattern-match textually.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Severity {
    // SCREAMING_SNAKE_CASE aliases so the `record!` macro can mirror
    // `tracing::Level::INFO` syntax at the call site (and so the macro
    // body's `$crate::Severity::$level` token forwarding works).
    pub const TRACE: Self = Self::Trace;
    pub const DEBUG: Self = Self::Debug;
    pub const INFO: Self = Self::Info;
    pub const WARN: Self = Self::Warn;
    pub const ERROR: Self = Self::Error;

    /// OTel severity_number for the bucket's "primary" sub-level.
    #[must_use]
    pub fn number(self) -> u8 {
        match self {
            Self::Trace => 1,
            Self::Debug => 5,
            Self::Info => 9,
            Self::Warn => 13,
            Self::Error => 17,
        }
    }

    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }

    /// Convert from a `tracing::Level`.
    #[must_use]
    pub fn from_tracing_level(level: tracing::Level) -> Self {
        match level {
            tracing::Level::TRACE => Self::Trace,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::INFO => Self::Info,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::ERROR => Self::Error,
        }
    }
}

/// ECS-style event.category coarse axis. Drives the dashboard's "hide
/// internal noise by default" filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventCategory {
    /// Agent loop activity: LLM requests, agent_start, agent_end.
    Agent,
    /// Channel I/O: inbound messages, outbound sends, draft updates.
    Channel,
    /// Cron scheduler activity: cron_run, cron_schedule.
    Cron,
    /// Memory backend ops: store, recall, forget.
    Memory,
    /// Tool execution: tool_call_start, tool_call_result.
    Tool,
    /// Provider activity (transient retries, provider switches).
    Provider,
    /// Session lifecycle: session_open, session_close.
    Session,
    /// System-level events: daemon_start, reload, migration.
    System,
    /// Ops noise that operators don't need on the dashboard by default:
    /// heartbeats, sync retries, idle evictions. Hidden in the UI unless
    /// explicitly opted into.
    Internal,
}

impl EventCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Channel => "channel",
            Self::Cron => "cron",
            Self::Memory => "memory",
            Self::Tool => "tool",
            Self::Provider => "provider",
            Self::Session => "session",
            Self::System => "system",
            Self::Internal => "internal",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "agent" => Some(Self::Agent),
            "channel" => Some(Self::Channel),
            "cron" => Some(Self::Cron),
            "memory" => Some(Self::Memory),
            "tool" => Some(Self::Tool),
            "provider" => Some(Self::Provider),
            "session" => Some(Self::Session),
            "system" => Some(Self::System),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }
}

/// ECS event.outcome. Default unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    Success,
    Failure,
    Unknown,
}

impl EventOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "success" => Some(Self::Success),
            "failure" => Some(Self::Failure),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// ECS-style nested event descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDescriptor {
    pub category: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "is_unknown_outcome")]
    pub outcome: String,
}

fn is_unknown_outcome(s: &String) -> bool {
    s == "unknown" || s.is_empty()
}

/// Service-identifier block. Constant for the daemon's lifetime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDescriptor {
    pub name: String,
    pub version: String,
}

impl Default for ServiceDescriptor {
    fn default() -> Self {
        Self {
            name: "zeroclaw".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// ZeroClaw-domain attribution fields. Every field is alias-bound where
/// applicable: `channel` is `<type>.<alias>` composite, `model_provider`
/// is `<type>.<alias>`, etc. Each composite also has its decomposed
/// pieces so filters can match either coarse or precise.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZeroclawAttribution {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_alias: Option<String>,

    /// Composite `<type>.<alias>`, e.g. `"discord.clamps"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_alias: Option<String>,

    /// Composite `<type>.<alias>`, e.g. `"anthropic.clamps"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_alias: Option<String>,

    /// Model name (provider-scoped, e.g. `"claude-sonnet-4-6"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Tool name when applicable (e.g. `"shell"`, `"file_read"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,

    /// Conversation session key (channel-scoped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,

    /// Cron job id (UUID) when the event was emitted from cron flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_job_id: Option<String>,

    /// Per-event duration when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl ZeroclawAttribution {
    /// Split a composite `<type>.<alias>` into its parts and populate the
    /// channel + channel_type + channel_alias fields in one call.
    pub fn set_channel_composite(&mut self, composite: &str) {
        self.channel = Some(composite.to_string());
        if let Some((ty, alias)) = composite.split_once('.') {
            self.channel_type = Some(ty.to_string());
            self.channel_alias = Some(alias.to_string());
        } else {
            // Bare type (e.g. legacy single-instance channel, or non-aliased
            // channel like webhook/cli). Type is the whole thing, alias is
            // absent.
            self.channel_type = Some(composite.to_string());
            self.channel_alias = None;
        }
    }

    pub fn set_model_provider_composite(&mut self, composite: &str) {
        self.model_provider = Some(composite.to_string());
        if let Some((ty, alias)) = composite.split_once('.') {
            self.model_provider_type = Some(ty.to_string());
            self.model_provider_alias = Some(alias.to_string());
        } else {
            self.model_provider_type = Some(composite.to_string());
            self.model_provider_alias = None;
        }
    }
}

/// One row in the canonical log stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    /// Persistent event id. UUID v4.
    pub id: String,

    /// RFC 3339 UTC timestamp with milliseconds. Keyed `@timestamp` to
    /// match ECS conventions; consumers (and our paginated reader) sort
    /// by this lexicographically, which works because RFC 3339 is sortable
    /// as a string.
    #[serde(rename = "@timestamp")]
    pub timestamp: String,

    pub severity_number: u8,
    pub severity_text: String,

    pub event: EventDescriptor,

    #[serde(default)]
    pub service: ServiceDescriptor,

    /// Per-turn trace identifier so multiple events from one agent
    /// turn group together in the UI. Hex string; populated by the
    /// agent loop at run() entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,

    /// Sub-span within a turn (e.g. one tool call inside a multi-tool
    /// iteration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,

    /// All the alias-bound attribution fields live here.
    #[serde(default)]
    pub zeroclaw: ZeroclawAttribution,

    /// Human-readable short message. The structured fields above carry the
    /// machine-readable detail; `message` is what a terminal-formatter
    /// prints as the line body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Free-form structured payload. Per-action contributors put extra
    /// data here (tokens used, iteration counter, tool input/output
    /// payloads when `log_tool_io` is enabled, anyhow error chain when
    /// the event is an error, …).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub attributes: Value,

    /// Schema version. `2` = this struct. Older files containing version-1
    /// rows get migrated in place at daemon startup.
    #[serde(default = "default_schema_version")]
    pub schema_version: u8,
}

fn default_schema_version() -> u8 {
    LogEvent::SCHEMA_VERSION
}

impl LogEvent {
    pub const SCHEMA_VERSION: u8 = 2;

    /// Build a fresh event with the given level + action + category.
    /// Caller fills in attribution and message before emission.
    #[must_use]
    pub fn new(severity: Severity, action: &str, category: EventCategory) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            severity_number: severity.number(),
            severity_text: severity.text().to_string(),
            event: EventDescriptor {
                category: category.as_str().to_string(),
                action: action.to_string(),
                outcome: EventOutcome::Unknown.as_str().to_string(),
            },
            service: ServiceDescriptor::default(),
            trace_id: None,
            span_id: None,
            zeroclaw: ZeroclawAttribution::default(),
            message: None,
            attributes: Value::Null,
            schema_version: LogEvent::SCHEMA_VERSION,
        }
    }

    pub fn set_outcome(&mut self, outcome: EventOutcome) {
        self.event.outcome = outcome.as_str().to_string();
    }
}

/// Lookup helper used by callers that already have an OTel-style severity
/// number and want the text bucket.
#[must_use]
pub fn severity_text_from_number(n: u8) -> &'static str {
    match n {
        0..=4 => "TRACE",
        5..=8 => "DEBUG",
        9..=12 => "INFO",
        13..=16 => "WARN",
        17..=20 => "ERROR",
        _ => "FATAL",
    }
}

#[must_use]
pub fn severity_text_from_tracing_level(level: tracing::Level) -> &'static str {
    Severity::from_tracing_level(level).text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_round_trip_through_tracing() {
        for (level, severity) in [
            (tracing::Level::TRACE, Severity::Trace),
            (tracing::Level::DEBUG, Severity::Debug),
            (tracing::Level::INFO, Severity::Info),
            (tracing::Level::WARN, Severity::Warn),
            (tracing::Level::ERROR, Severity::Error),
        ] {
            assert_eq!(Severity::from_tracing_level(level), severity);
        }
    }

    #[test]
    fn severity_text_buckets_match_number() {
        assert_eq!(severity_text_from_number(1), "TRACE");
        assert_eq!(severity_text_from_number(5), "DEBUG");
        assert_eq!(severity_text_from_number(9), "INFO");
        assert_eq!(severity_text_from_number(13), "WARN");
        assert_eq!(severity_text_from_number(17), "ERROR");
        assert_eq!(severity_text_from_number(22), "FATAL");
    }

    #[test]
    fn set_channel_composite_splits() {
        let mut z = ZeroclawAttribution::default();
        z.set_channel_composite("discord.clamps");
        assert_eq!(z.channel.as_deref(), Some("discord.clamps"));
        assert_eq!(z.channel_type.as_deref(), Some("discord"));
        assert_eq!(z.channel_alias.as_deref(), Some("clamps"));
    }

    #[test]
    fn set_channel_composite_bare_type() {
        let mut z = ZeroclawAttribution::default();
        z.set_channel_composite("webhook");
        assert_eq!(z.channel_type.as_deref(), Some("webhook"));
        assert!(z.channel_alias.is_none());
    }

    #[test]
    fn set_model_provider_composite_splits() {
        let mut z = ZeroclawAttribution::default();
        z.set_model_provider_composite("anthropic.clamps");
        assert_eq!(z.model_provider.as_deref(), Some("anthropic.clamps"));
        assert_eq!(z.model_provider_type.as_deref(), Some("anthropic"));
        assert_eq!(z.model_provider_alias.as_deref(), Some("clamps"));
    }

    #[test]
    fn event_serializes_with_at_timestamp_key() {
        let e = LogEvent::new(Severity::Info, "test", EventCategory::Agent);
        let v = serde_json::to_value(&e).unwrap();
        assert!(v.get("@timestamp").is_some());
        assert!(v.get("timestamp").is_none());
        assert_eq!(v["severity_text"], "INFO");
        assert_eq!(v["severity_number"], 9);
        assert_eq!(v["event"]["category"], "agent");
        assert_eq!(v["event"]["action"], "test");
        assert_eq!(v["schema_version"], LogEvent::SCHEMA_VERSION);
    }

    #[test]
    fn unknown_outcome_omitted_from_serialization() {
        let e = LogEvent::new(Severity::Info, "test", EventCategory::Agent);
        let v = serde_json::to_value(&e).unwrap();
        assert!(v["event"].get("outcome").is_none());
    }
}
