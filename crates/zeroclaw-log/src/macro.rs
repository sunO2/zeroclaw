//! The `record!` macro — the single emission point every other crate
//! reaches for to log a semantic ZeroClaw event.
//!
//! Usage:
//!
//! ```ignore
//! use zeroclaw_log::record;
//!
//! record!(
//!     INFO,
//!     action: "llm_request",
//!     category: "agent",
//!     outcome: "success",
//!     agent: agent_alias,
//!     channel: "discord.clamps",
//!     model_provider: "anthropic.clamps",
//!     model: "claude-sonnet-4-6",
//!     trace_id: turn_id,
//!     duration_ms: 412u64,
//!     message: "LLM request completed",
//!     attrs: serde_json::json!({ "tokens": 412, "messages": 3 }),
//! );
//! ```
//!
//! Implementation: top-level macro takes the level + a recognized-key
//! body. Each key is consumed one at a time via a recursive helper, so
//! values can be heterogeneous (`&str`, `String`, `u64`, `serde_json::Value`,
//! `&anyhow::Error`, …) without the walker forcing them through a single
//! `Option<&str>` like the previous design did.
//!
//! Unknown keys are a compile error in the per-key arm — typos fail
//! loud, not silently.
//!
//! Fan-out from one call: tracing event + persisted JSONL + broadcast.

/// Build and emit a structured ZeroClaw log event. See module docs for
/// recognized fields and semantics.
#[macro_export]
macro_rules! record {
    ($level:ident, $($body:tt)+) => {{
        // Resolve the level + extract action/category from the body.
        let __severity = $crate::Severity::$level;
        let mut __action: &'static str = "unknown";
        let mut __category: $crate::EventCategory = $crate::EventCategory::System;
        $crate::__record_extract_meta!(__action, __category; $($body)+);

        let mut __ev = $crate::LogEvent::new(__severity, __action, __category);
        $crate::__record_apply!(__ev; $($body)+);

        // Forward to tracing at the matching level. The macro expands at
        // the caller's site, so `RUST_LOG=zeroclaw_runtime::agent=debug`
        // filtering targets the actual emitting module path.
        $crate::tracing::event!(
            $crate::tracing::Level::$level,
            event = __action,
        );

        $crate::record_event(__ev);
    }};
}

// Walk the body, extracting `action` and `category` only. Unknown keys
// are skipped (the apply-walker enforces the whitelist).
#[doc(hidden)]
#[macro_export]
macro_rules! __record_extract_meta {
    ($action:ident, $category:ident; action: $v:expr $(, $($rest:tt)*)?) => {
        $action = $v;
        $crate::__record_extract_meta!($action, $category; $($($rest)*)?);
    };
    ($action:ident, $category:ident; category: $v:expr $(, $($rest:tt)*)?) => {
        $category = $crate::EventCategory::parse($v).unwrap_or($crate::EventCategory::System);
        $crate::__record_extract_meta!($action, $category; $($($rest)*)?);
    };
    ($action:ident, $category:ident; $unknown:ident: $v:expr $(, $($rest:tt)*)?) => {
        let _ = $v;
        $crate::__record_extract_meta!($action, $category; $($($rest)*)?);
    };
    ($action:ident, $category:ident;) => {};
}

// Walk the body, applying each recognized key to the event. Unknown
// keys hit the catch-all and trip a compile error.
#[doc(hidden)]
#[macro_export]
macro_rules! __record_apply {
    ($ev:ident; action: $v:expr $(, $($rest:tt)*)?) => {
        // Applied in record! itself when constructing the event.
        let _ = $v;
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; category: $v:expr $(, $($rest:tt)*)?) => {
        let _ = $v;
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; outcome: $v:expr $(, $($rest:tt)*)?) => {
        if let Some(__o) = $crate::EventOutcome::parse($v) {
            $ev.set_outcome(__o);
        }
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; message: $v:expr $(, $($rest:tt)*)?) => {
        $ev.message = Some(($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; agent: $v:expr $(, $($rest:tt)*)?) => {
        $ev.zeroclaw.agent_alias = Some(($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; channel: $v:expr $(, $($rest:tt)*)?) => {
        $ev.zeroclaw.set_channel_composite(&($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; model_provider: $v:expr $(, $($rest:tt)*)?) => {
        $ev.zeroclaw.set_model_provider_composite(&($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; model: $v:expr $(, $($rest:tt)*)?) => {
        $ev.zeroclaw.model = Some(($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; tool: $v:expr $(, $($rest:tt)*)?) => {
        $ev.zeroclaw.tool = Some(($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; session_key: $v:expr $(, $($rest:tt)*)?) => {
        $ev.zeroclaw.session_key = Some(($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; cron_job_id: $v:expr $(, $($rest:tt)*)?) => {
        $ev.zeroclaw.cron_job_id = Some(($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; duration_ms: $v:expr $(, $($rest:tt)*)?) => {
        $ev.zeroclaw.duration_ms = Some(($v) as u64);
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; trace_id: $v:expr $(, $($rest:tt)*)?) => {
        $ev.trace_id = Some(($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; span_id: $v:expr $(, $($rest:tt)*)?) => {
        $ev.span_id = Some(($v).to_string());
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; attrs: $v:expr $(, $($rest:tt)*)?) => {
        $ev.attributes = $v;
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    ($ev:ident; error: $v:expr $(, $($rest:tt)*)?) => {
        let __chain = $crate::display_chain($v);
        if $ev.message.is_none() {
            $ev.message = Some(__chain.clone());
        }
        let __attrs_prev = std::mem::take(&mut $ev.attributes);
        $ev.attributes = match __attrs_prev {
            $crate::serde_json::Value::Null => $crate::serde_json::json!({ "error_chain": __chain }),
            $crate::serde_json::Value::Object(mut __m) => {
                __m.insert(
                    "error_chain".into(),
                    $crate::serde_json::Value::String(__chain),
                );
                $crate::serde_json::Value::Object(__m)
            }
            __other => $crate::serde_json::json!({ "error_chain": __chain, "previous": __other }),
        };
        $crate::__record_apply!($ev; $($($rest)*)?);
    };
    // Terminal — no more body left.
    ($ev:ident;) => {};
    // Catch-all: an unrecognized key. Surface as a compile error so
    // typos fail loud.
    ($ev:ident; $unknown:ident: $v:expr $(, $($rest:tt)*)?) => {
        compile_error!(concat!(
            "zeroclaw_log::record!: unknown field `",
            stringify!($unknown),
            "`. Recognized: action, category, outcome, message, agent, channel, model_provider, model, tool, session_key, cron_job_id, duration_ms, trace_id, span_id, attrs, error."
        ));
    };
}

#[cfg(test)]
mod tests {
    use crate::Severity;

    #[test]
    fn macro_emits_basic_event_through_writer() {
        let _guard = crate::writer::WRITER_TEST_LOCK.lock();
        let tmp = tempfile::tempdir().unwrap();
        let cfg = zeroclaw_config::schema::ObservabilityConfig {
            log_persistence: "full".into(),
            ..zeroclaw_config::schema::ObservabilityConfig::default()
        };
        crate::init_from_config(&cfg, tmp.path());

        let agent_alias = "clamps";
        record!(
            INFO,
            action: "test_event",
            category: "agent",
            agent: agent_alias,
            channel: "discord.clamps",
            message: "hello",
            attrs: serde_json::json!({ "tokens": 412 }),
        );

        let path = crate::runtime_trace_path().unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let line = contents.lines().next().unwrap();
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["event"]["action"], "test_event");
        assert_eq!(v["event"]["category"], "agent");
        assert_eq!(v["zeroclaw"]["agent_alias"], "clamps");
        assert_eq!(v["zeroclaw"]["channel"], "discord.clamps");
        assert_eq!(v["zeroclaw"]["channel_type"], "discord");
        assert_eq!(v["zeroclaw"]["channel_alias"], "clamps");
        assert_eq!(v["message"], "hello");
        assert_eq!(v["severity_text"], "INFO");
        assert_eq!(v["severity_number"], Severity::Info.number() as i64);
        assert_eq!(v["attributes"]["tokens"], 412);
    }
}
