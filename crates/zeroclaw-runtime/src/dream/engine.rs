//! Dream mode engine — periodic memory consolidation and reflective learning.
//!
//! During idle periods, the dream engine:
//! 1. **Gathers** recent daily memories and conversation summaries.
//! 2. **Reflects** via a single LLM pass to identify patterns and insights.
//! 3. **Consolidates** distilled insights into long-term Core memories.
//! 4. **Prunes** stale daily memories and low-importance entries.
//! 5. **Reports** an optional summary for the next user interaction.
//!
//! The engine runs as a daemon component (like heartbeat) with its own
//! supervisor. Only the Reflect phase calls the LLM; all other phases
//! operate directly on the Memory trait to minimize token cost.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info, warn};
use zeroclaw_api::memory_traits::{Memory, MemoryCategory, MemoryEntry};
use zeroclaw_api::provider::Provider;
use zeroclaw_config::schema::DreamModeConfig;

use super::report::DreamReport;

// ── Dream cycle result ─────────────────────────────────────────

/// Result of a single dream cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamCycleResult {
    /// Number of memories gathered for reflection.
    pub gathered_count: usize,
    /// Insights distilled by the LLM reflect phase.
    pub insights: Vec<String>,
    /// Number of stale memories pruned.
    pub pruned_count: usize,
    /// Number of new Core memories created from insights.
    pub consolidated_count: usize,
    /// Summary text for the "While you were away..." report.
    pub report_summary: Option<String>,
    /// Timestamp of the dream cycle.
    pub timestamp: DateTime<Utc>,
}

// ── LLM prompt ─────────────────────────────────────────────────

const DREAM_REFLECT_SYSTEM_PROMPT: &str = r#"You are a memory consolidation engine performing a "dream cycle". Given a collection of recent daily memories and conversation summaries, analyze them and extract:

1. "insights": An array of concise, actionable insights — patterns you notice, recurring topics, preference shifts, important decisions, or lessons learned. Each insight should be a single sentence that stands alone as a useful fact to remember long-term.

2. "stale_keys": An array of memory keys that appear outdated, contradictory, or superseded by newer information. Only include keys you are confident are no longer relevant.

3. "summary": A 1-3 sentence natural language summary of what happened during this period, suitable for a "While you were away..." report.

Respond ONLY with valid JSON:
{"insights": ["...", "..."], "stale_keys": ["..."], "summary": "..."}
Do not include any text outside the JSON object."#;

/// Parsed output from the LLM reflect phase.
#[derive(Debug, Deserialize)]
struct ReflectResult {
    #[serde(default)]
    insights: Vec<String>,
    #[serde(default)]
    stale_keys: Vec<String>,
    #[serde(default)]
    summary: Option<String>,
}

// ── Engine ──────────────────────────────────────────────────────

/// Dream mode engine — consolidates memories during idle periods.
pub struct DreamEngine {
    config: DreamModeConfig,
    workspace_dir: PathBuf,
}

impl DreamEngine {
    pub fn new(config: DreamModeConfig, workspace_dir: PathBuf) -> Self {
        Self {
            config,
            workspace_dir,
        }
    }

    /// Run a single dream cycle: gather → reflect → consolidate → prune → report.
    ///
    /// This is the main entry point for both scheduled and manual triggers.
    pub async fn run_cycle(
        &self,
        memory: &dyn Memory,
        provider: &dyn Provider,
        model: &str,
    ) -> Result<DreamCycleResult> {
        info!("Dream cycle started");

        // Phase 1: Gather
        let gathered = self.gather(memory).await?;
        let gathered_count = gathered.len();
        info!("Dream gather: {} memories collected", gathered_count);

        if gathered.is_empty() {
            info!("Dream cycle skipped: no recent memories to consolidate");
            return Ok(DreamCycleResult {
                gathered_count: 0,
                insights: Vec::new(),
                pruned_count: 0,
                consolidated_count: 0,
                report_summary: None,
                timestamp: Utc::now(),
            });
        }

        // Phase 2: Reflect (LLM call)
        let reflect_result = self.reflect(provider, model, &gathered).await?;
        info!(
            "Dream reflect: {} insights, {} stale keys",
            reflect_result.insights.len(),
            reflect_result.stale_keys.len()
        );

        // Phase 3: Consolidate insights into Core memories
        let consolidated_count = self.consolidate(memory, &reflect_result.insights).await?;
        info!(
            "Dream consolidate: {} new Core memories",
            consolidated_count
        );

        // Phase 4: Prune stale memories
        let pruned_count = self.prune(memory, &reflect_result.stale_keys).await?;
        info!("Dream prune: {} memories removed", pruned_count);

        // Phase 5: Build report
        let result = DreamCycleResult {
            gathered_count,
            insights: reflect_result.insights,
            pruned_count,
            consolidated_count,
            report_summary: reflect_result.summary,
            timestamp: Utc::now(),
        };

        // Persist report for "While you were away..." display
        if self.config.show_report
            && let Some(ref summary) = result.report_summary
        {
            let report = DreamReport {
                summary: summary.clone(),
                insights_count: result.consolidated_count,
                pruned_count: result.pruned_count,
                timestamp: result.timestamp,
                delivered: false,
            };
            if let Err(e) = report.save(&self.workspace_dir) {
                warn!("Failed to persist dream report: {e}");
            }
        }

        info!(
            "Dream cycle complete: {} insights, {} pruned",
            result.consolidated_count, result.pruned_count
        );

        Ok(result)
    }

    // ── Phase 1: Gather ────────────────────────────────────────

    /// Collect recent daily memories and conversation summaries.
    async fn gather(&self, memory: &dyn Memory) -> Result<Vec<MemoryEntry>> {
        // Compute the time window: gather memories from the last 24 hours
        // (or since the last dream cycle, whichever is more recent).
        let since = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();

        let mut entries = memory
            .recall("", self.config.gather_limit, None, Some(&since), None)
            .await
            .context("dream gather: failed to recall recent memories")?;

        // Exclude Conversation memories to avoid chat context leaking into dreams.
        entries.retain(|e| !matches!(e.category, MemoryCategory::Conversation));

        // Cap to configured limit.
        entries.truncate(self.config.gather_limit);

        Ok(entries)
    }

    // ── Phase 2: Reflect ───────────────────────────────────────

    /// Single LLM pass to identify patterns, insights, and stale entries.
    async fn reflect(
        &self,
        provider: &dyn Provider,
        model: &str,
        gathered: &[MemoryEntry],
    ) -> Result<ReflectResult> {
        // Build the input text from gathered memories.
        let mut input = String::with_capacity(gathered.len() * 200);
        for entry in gathered {
            input.push_str(&format!(
                "[{}] ({}) {}: {}\n",
                entry.timestamp, entry.category, entry.key, entry.content
            ));
        }

        // Truncate to avoid excessive token cost.
        let truncated = truncate_utf8(&input, 8000);

        let raw = provider
            .chat_with_system(
                Some(DREAM_REFLECT_SYSTEM_PROMPT),
                &truncated,
                model,
                Some(self.config.temperature),
            )
            .await
            .context("dream reflect: LLM call failed")?;

        parse_reflect_response(&raw)
    }

    // ── Phase 3: Consolidate ───────────────────────────────────

    /// Store distilled insights as Core memories with importance scoring.
    async fn consolidate(&self, memory: &dyn Memory, insights: &[String]) -> Result<usize> {
        let mut stored = 0;

        for insight in insights {
            if insight.trim().is_empty() {
                continue;
            }

            let key = format!("dream_insight_{}", uuid::Uuid::new_v4());
            let importance =
                zeroclaw_memory::importance::compute_importance(insight, &MemoryCategory::Core);

            // Check for conflicts with existing Core memories.
            if let Err(e) = zeroclaw_memory::conflict::check_and_resolve_conflicts(
                memory,
                &key,
                insight,
                &MemoryCategory::Core,
                0.85,
            )
            .await
            {
                debug!("dream consolidate: conflict check skipped: {e}");
            }

            if self.config.audit_mode {
                // In audit mode, don't persist — just count. The caller
                // should serialize the DreamCycleResult for review.
                stored += 1;
                continue;
            }

            match memory
                .store_with_metadata(
                    &key,
                    insight,
                    MemoryCategory::Core,
                    None,
                    Some("dream"),
                    Some(importance),
                )
                .await
            {
                Ok(()) => stored += 1,
                Err(e) => {
                    warn!("dream consolidate: failed to store insight: {e}");
                }
            }
        }

        Ok(stored)
    }

    // ── Phase 4: Prune ─────────────────────────────────────────

    /// Remove stale daily memories and LLM-identified outdated entries.
    async fn prune(&self, memory: &dyn Memory, stale_keys: &[String]) -> Result<usize> {
        if self.config.audit_mode {
            return Ok(stale_keys.len());
        }

        let mut pruned = 0;

        // Remove LLM-identified stale entries.
        for key in stale_keys {
            match memory.forget(key).await {
                Ok(true) => pruned += 1,
                Ok(false) => {
                    debug!("dream prune: key not found: {key}");
                }
                Err(e) => {
                    debug!("dream prune: failed to forget key {key}: {e}");
                }
            }
        }

        // Prune old Daily memories beyond max_daily_age_days.
        let cutoff = (Utc::now()
            - chrono::Duration::days(i64::from(self.config.max_daily_age_days)))
        .to_rfc3339();

        match memory.list(Some(&MemoryCategory::Daily), None).await {
            Ok(entries) => {
                for entry in entries {
                    if entry.timestamp.as_str() < cutoff.as_str()
                        && let Ok(true) = memory.forget(&entry.key).await
                    {
                        pruned += 1;
                    }
                }
            }
            Err(e) => {
                debug!("dream prune: failed to list daily memories: {e}");
            }
        }

        Ok(pruned)
    }
}

// ── Helpers ────────────────────────────────────────────────────

/// Truncate a string to at most `max_bytes` at a valid UTF-8 char boundary.
fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= max_bytes)
        .last()
        .unwrap_or(0);
    format!("{}...", &s[..end])
}

/// Parse the LLM's reflect response, with fallback for malformed JSON.
fn parse_reflect_response(raw: &str) -> Result<ReflectResult> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str(cleaned).map_err(|e| {
        debug!("dream reflect: failed to parse LLM response: {e}");
        debug!("dream reflect: raw response: {raw}");
        anyhow::anyhow!("dream reflect: malformed LLM response: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_utf8_handles_ascii() {
        assert_eq!(truncate_utf8("hello world", 5), "hello...");
        assert_eq!(truncate_utf8("short", 100), "short");
    }

    #[test]
    fn truncate_utf8_handles_multibyte() {
        let s = "hello \u{1F600} world";
        let result = truncate_utf8(s, 7);
        assert!(result.is_char_boundary(result.len() - 3)); // "..." suffix
    }

    #[test]
    fn parse_reflect_response_valid_json() {
        let raw = r#"{"insights": ["User prefers Rust"], "stale_keys": ["old_key"], "summary": "Quiet day"}"#;
        let result = parse_reflect_response(raw).unwrap();
        assert_eq!(result.insights.len(), 1);
        assert_eq!(result.stale_keys.len(), 1);
        assert_eq!(result.summary.as_deref(), Some("Quiet day"));
    }

    #[test]
    fn parse_reflect_response_code_fenced() {
        let raw = "```json\n{\"insights\": [], \"stale_keys\": [], \"summary\": null}\n```";
        let result = parse_reflect_response(raw).unwrap();
        assert!(result.insights.is_empty());
    }

    #[test]
    fn parse_reflect_response_invalid_returns_error() {
        let raw = "not json at all";
        assert!(parse_reflect_response(raw).is_err());
    }

    #[test]
    fn dream_cycle_result_serializes() {
        let result = DreamCycleResult {
            gathered_count: 10,
            insights: vec!["pattern A".into()],
            pruned_count: 2,
            consolidated_count: 1,
            report_summary: Some("Summary".into()),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("pattern A"));
    }
}
