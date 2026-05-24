//! Shared request/response types for the ZeroClaw RPC + gateway API surface.
//!
//! **Single source of truth.** Every domain's wire types live here.
//! The RPC dispatcher, the HTTP gateway, and the TUI client all
//! import from this module. No ad-hoc `json!()`, no duplicated structs.
//!
//! ## Conventions
//!
//! - All structs derive `Debug, Clone, Serialize, Deserialize`.
//! - All structs use `#[serde(rename_all = "snake_case")]`.
//! - Optional fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`.
//! - Types that already exist elsewhere (`MemoryEntry`, `CronJob`,
//!   `CostSummary`, `SkillFrontmatter`) are re-exported, not re-defined.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Re-exports: types that already derive Serialize + Deserialize ────
// Consumers can `use zeroclaw_runtime::rpc::types::*` and get everything.

pub use crate::cron::{CronJob, CronJobPatch, CronRun, DeliveryConfig, Schedule};
pub use crate::rpc::session::SessionOverrides;
pub use crate::skills::frontmatter::SkillFrontmatter;
pub use zeroclaw_api::memory_traits::{MemoryCategory, MemoryEntry};
pub use zeroclaw_config::cost::types::CostSummary;
pub use zeroclaw_config::traits::{ConfigFieldEntry, PropKind};

// ── Derive helper ────────────────────────────────────────────────────

macro_rules! rpc_type {
    (
        $(#[$meta:meta])*
        pub struct $name:ident { $($body:tt)* }
    ) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        $(#[$meta])*
        pub struct $name { $($body)* }
    };
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $($body:tt)* }
    ) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        $(#[$meta])*
        pub enum $name { $($body)* }
    };
}

// ══════════════════════════════════════════════════════════════════════
// ── Core ─────────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    pub struct InitializeParams {
        #[serde(default = "default_protocol_version")]
        pub protocol_version: u64,
        /// TUI ID from a previous connection (reconnection).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tui_id: Option<String>,
        /// HMAC signature proving ownership of the claimed TUI ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tui_sig: Option<String>,
        /// Shell environment from the TUI process, used to forward the user's
        /// real env (PATH, credentials, etc.) to subprocesses spawned by the
        /// daemon on their behalf. Omitted by older clients; defaults to empty.
        #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
        pub env: std::collections::HashMap<String, String>,
    }
}

fn default_protocol_version() -> u64 {
    1
}

rpc_type! {
    pub struct InitializeResult {
        pub protocol_version: u64,
        pub server_version: String,
        /// Assigned TUI session UID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tui_id: Option<String>,
        /// HMAC signature for reconnection. Pass back in next initialize.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tui_sig: Option<String>,
        /// Supported RPC method names (e.g. "session/prompt", "memory/list").
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub capabilities: Vec<String>,
    }
}

rpc_type! {
    pub struct StatusResult {
        pub server_version: String,
        pub protocol_version: u64,
        pub active_sessions: usize,
        pub session_ids: Vec<String>,
    }
}

// Health: no params, result is `Value` from `health::snapshot_json()`.

// ══════════════════════════════════════════════════════════════════════
// ── TUI ──────────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    pub struct TuiListEntry {
        pub tui_id: String,
        /// RFC 3339 timestamp (for gateway API / web frontend).
        pub connected_at: String,
        /// Unix epoch seconds (for TUI client relative-time display
        /// without requiring chrono).
        pub connected_at_unix: i64,
        pub peer_label: String,
        /// Transport protocol: `"unix"` or `"wss"`.
        pub transport: String,
    }
}

rpc_type! {
    pub struct TuiListResult {
        pub tuis: Vec<TuiListEntry>,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Sessions ─────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    /// Shared param for methods that only need a session ID:
    /// `session/close`, `session/cancel`, `session/messages`,
    /// `session/state`, `session/delete`.
    pub struct SessionIdParams {
        pub session_id: String,
    }
}

rpc_type! {
    pub struct SessionNewParams {
        pub agent_alias: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub session_id: Option<String>,
    }
}

rpc_type! {
    pub struct SessionNewResult {
        pub session_id: String,
        pub agent_alias: String,
        pub message_count: usize,
        pub workspace_dir: String,
    }
}

rpc_type! {
    pub struct SessionCloseResult {
        pub session_id: String,
        pub closed: bool,
    }
}

rpc_type! {
    pub struct SessionPromptParams {
        pub session_id: String,
        pub prompt: String,
        /// Inline file attachments. Processed identically to `file/attach`
        /// entries — markers are appended to the prompt before the turn runs.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub attachments: Vec<FileEntry>,
    }
}

rpc_type! {
    pub struct SessionPromptResult {
        pub session_id: String,
        pub stop_reason: String,
        pub content: String,
    }
}

rpc_type! {
    pub struct SessionConfigureParams {
        pub session_id: String,
        #[serde(default)]
        pub overrides: SessionOverrides,
    }
}

rpc_type! {
    pub struct SessionConfigureResult {
        pub session_id: String,
        pub overrides: SessionOverrides,
    }
}

rpc_type! {
    pub struct SessionCancelResult {
        pub session_id: String,
        pub cancelled: bool,
    }
}

rpc_type! {
    pub struct SessionListParams {
        /// Full-text search query. When present, only sessions whose message
        /// content matches (via FTS5) are returned.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub query: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub limit: Option<usize>,
    }
}

rpc_type! {
    pub struct SessionListResult {
        pub sessions: Vec<SessionEntry>,
    }
}

rpc_type! {
    pub struct SessionEntry {
        pub session_id: String,
        pub session_key: String,
        pub created_at: String,
        pub last_activity: String,
        pub message_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent_alias: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub channel_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
    }
}

rpc_type! {
    pub struct SessionMessagesResult {
        pub session_id: String,
        pub messages: Vec<MessageEntry>,
    }
}

rpc_type! {
    pub struct MessageEntry {
        pub role: String,
        pub content: String,
    }
}

rpc_type! {
    pub struct SessionStateResult {
        pub session_id: String,
        pub state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub turn_started_at: Option<String>,
    }
}

rpc_type! {
    pub struct SessionDeleteResult {
        pub session_id: String,
        pub deleted: bool,
    }
}

rpc_type! {
    pub struct SessionRenameParams {
        pub session_id: String,
        pub name: String,
    }
}

rpc_type! {
    pub struct SessionRenameResult {
        pub session_id: String,
        pub name: String,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Memory ───────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    /// Params for `memory/list`. Consolidates gateway `MemoryQuery` (list mode).
    pub struct MemoryListParams {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub category: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent: Option<String>,
    }
}

rpc_type! {
    pub struct MemoryListResult {
        pub entries: Vec<MemoryEntry>,
        pub count: usize,
    }
}

rpc_type! {
    /// Params for `memory/search`. Consolidates gateway `MemoryQuery` (search mode).
    pub struct MemorySearchParams {
        pub query: String,
        #[serde(default = "default_search_limit")]
        pub limit: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub since: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub until: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent: Option<String>,
    }
}

fn default_search_limit() -> usize {
    10
}

rpc_type! {
    pub struct MemorySearchResult {
        pub entries: Vec<MemoryEntry>,
        pub count: usize,
    }
}

rpc_type! {
    /// Params for `memory/store`. Consolidates gateway `MemoryStoreBody`.
    pub struct MemoryStoreParams {
        pub key: String,
        pub content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub category: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent: Option<String>,
    }
}

rpc_type! {
    pub struct MemoryStoreResult {
        pub key: String,
        pub stored: bool,
    }
}

rpc_type! {
    /// Params for `memory/delete`. Consolidates gateway `MemoryDeleteQuery`.
    pub struct MemoryDeleteParams {
        pub key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent: Option<String>,
    }
}

rpc_type! {
    pub struct MemoryDeleteResult {
        pub key: String,
        pub deleted: bool,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Cron ─────────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    pub struct CronListResult {
        pub jobs: Vec<CronJob>,
    }
}

rpc_type! {
    pub struct CronIdParams {
        pub id: String,
    }
}

rpc_type! {
    /// Params for `cron/add`. Consolidates gateway `CronAddBody`.
    pub struct CronAddParams {
        pub agent: String,
        pub schedule: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tz: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub job_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub delivery: Option<DeliveryConfig>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub session_target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub allowed_tools: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub delete_after_run: Option<bool>,
    }
}

rpc_type! {
    /// Params for `cron/patch`. Consolidates gateway `CronPatchBody`.
    pub struct CronPatchParams {
        pub id: String,
        pub agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub schedule: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tz: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub clear_tz: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub prompt: Option<String>,
    }
}

rpc_type! {
    pub struct CronDeleteResult {
        pub id: String,
        pub deleted: bool,
    }
}

rpc_type! {
    pub struct CronRunsParams {
        pub id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub limit: Option<u32>,
    }
}

rpc_type! {
    pub struct CronRunsResult {
        pub runs: Vec<CronRun>,
    }
}

rpc_type! {
    pub struct CronTriggerResult {
        pub id: String,
        pub success: bool,
        pub output: String,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Config ───────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    pub struct ConfigGetParams {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub prop: Option<String>,
    }
}

rpc_type! {
    /// Returned when `config/get` is called with a specific `prop`.
    pub struct ConfigGetPropResult {
        pub prop: String,
        pub value: String,
    }
}

// Full config read returns `Value` (masked) — inherently untyped.

rpc_type! {
    /// Value is polymorphic: a JSON string passes through as-is (backward
    /// compat); any other JSON type is coerced via `coerce_for_set_prop`.
    pub struct ConfigSetParams {
        pub prop: String,
        pub value: Value,
    }
}

rpc_type! {
    pub struct ConfigSetResult {
        pub prop: String,
        pub set: bool,
    }
}

rpc_type! {
    pub struct ConfigValidateResult {
        pub valid: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub error: Option<String>,
    }
}

rpc_type! {
    pub struct ConfigReloadResult {
        pub reloading: bool,
    }
}

rpc_type! {
    pub struct ConfigListParams {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub prefix: Option<String>,
    }
}

rpc_type! {
    pub struct ConfigListResult {
        pub entries: Vec<ConfigFieldEntry>,
    }
}

rpc_type! {
    pub struct ConfigDeleteParams {
        pub prop: String,
    }
}

rpc_type! {
    pub struct ConfigDeleteResult {
        pub prop: String,
        pub deleted: bool,
    }
}

rpc_type! {
    pub struct ConfigMapKeysParams {
        pub path: String,
    }
}

rpc_type! {
    pub struct ConfigMapKeysResult {
        pub path: String,
        pub keys: Vec<String>,
    }
}

rpc_type! {
    pub struct ConfigMapKeyCreateParams {
        pub path: String,
        pub key: String,
    }
}

rpc_type! {
    pub struct ConfigMapKeyCreateResult {
        pub path: String,
        pub key: String,
        pub created: bool,
    }
}

rpc_type! {
    pub struct ConfigMapKeyDeleteParams {
        pub path: String,
        pub key: String,
    }
}

rpc_type! {
    pub struct ConfigMapKeyDeleteResult {
        pub path: String,
        pub key: String,
        pub deleted: bool,
    }
}

rpc_type! {
    pub struct ConfigMapKeyRenameParams {
        pub path: String,
        pub from: String,
        pub to: String,
    }
}

rpc_type! {
    pub struct ConfigMapKeyRenameResult {
        pub path: String,
        pub from: String,
        pub to: String,
        pub renamed: bool,
    }
}

rpc_type! {
    /// Owned wire representation of a [`zeroclaw_config::traits::MapKeySection`].
    /// The upstream type uses `&'static str` fields that can't round-trip
    /// through `Deserialize`, so this owned copy serves as the wire format.
    pub struct ConfigTemplateEntry {
        pub path: String,
        pub kind: zeroclaw_config::traits::MapKeyKind,
        pub value_type: String,
        pub description: String,
    }
}

impl From<zeroclaw_config::traits::MapKeySection> for ConfigTemplateEntry {
    fn from(s: zeroclaw_config::traits::MapKeySection) -> Self {
        Self {
            path: s.path.to_string(),
            kind: s.kind,
            value_type: s.value_type.to_string(),
            description: s.description.to_string(),
        }
    }
}

rpc_type! {
    pub struct ConfigTemplatesResult {
        pub templates: Vec<ConfigTemplateEntry>,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Agents ───────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    pub struct AgentEntry {
        pub alias: String,
        pub enabled: bool,
        pub channels: Vec<String>,
    }
}

rpc_type! {
    pub struct AgentsListResult {
        pub agents: Vec<AgentEntry>,
    }
}

rpc_type! {
    pub struct AgentStatusEntry {
        pub alias: String,
        pub enabled: bool,
        pub active_sessions: usize,
        #[serde(default)]
        pub channels: Vec<String>,
    }
}

rpc_type! {
    pub struct AgentsStatusResult {
        pub agents: Vec<AgentStatusEntry>,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Cost ─────────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    /// Params for `cost/query`. Consolidates gateway `CostQuery`.
    pub struct CostQueryParams {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub to: Option<String>,
    }
}

// Result is `CostSummary` directly (already Serialize).

// ══════════════════════════════════════════════════════════════════════
// ── Skills ───────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    /// Wire representation of a skill bundle. Consolidates gateway `BundleEntry`.
    pub struct SkillBundleEntry {
        pub alias: String,
        pub directory: String,
        pub include: Vec<String>,
        pub exclude: Vec<String>,
    }
}

rpc_type! {
    pub struct SkillsBundlesResult {
        pub bundles: Vec<SkillBundleEntry>,
    }
}

rpc_type! {
    pub struct SkillsListParams {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub bundle: Option<String>,
    }
}

rpc_type! {
    /// Wire representation of a skill in a list. Consolidates gateway `SkillEntry`.
    pub struct SkillListEntry {
        pub bundle: String,
        pub name: String,
        pub directory: String,
        pub frontmatter: SkillFrontmatter,
    }
}

rpc_type! {
    pub struct SkillsListResult {
        pub skills: Vec<SkillListEntry>,
    }
}

rpc_type! {
    pub struct SkillsReadParams {
        pub bundle: String,
        pub name: String,
    }
}

rpc_type! {
    /// Consolidates gateway `SkillReadResponse`.
    pub struct SkillsReadResult {
        pub bundle: String,
        pub name: String,
        pub frontmatter: SkillFrontmatter,
        pub body: String,
    }
}

rpc_type! {
    pub struct SkillsWriteParams {
        pub bundle: String,
        pub name: String,
        pub frontmatter: SkillFrontmatter,
        #[serde(default)]
        pub body: String,
    }
}

rpc_type! {
    pub struct SkillsWriteResult {
        pub bundle: String,
        pub name: String,
        pub written: bool,
    }
}

rpc_type! {
    pub struct SkillsDeleteParams {
        pub bundle: String,
        pub name: String,
    }
}

rpc_type! {
    pub struct SkillsDeleteResult {
        pub bundle: String,
        pub name: String,
        pub deleted: bool,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Personality ──────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    pub struct PersonalityListParams {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent: Option<String>,
    }
}

rpc_type! {
    /// Consolidates gateway `PersonalityIndexEntry`.
    pub struct PersonalityFileEntry {
        pub filename: String,
        pub exists: bool,
        #[serde(default)]
        pub size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub mtime_ms: Option<i64>,
    }
}

rpc_type! {
    /// Consolidates gateway `PersonalityIndex`.
    pub struct PersonalityListResult {
        pub files: Vec<PersonalityFileEntry>,
        pub max_chars: usize,
    }
}

rpc_type! {
    pub struct PersonalityGetParams {
        pub agent: String,
        pub filename: String,
    }
}

rpc_type! {
    /// Consolidates gateway `PersonalityFileResponse`.
    pub struct PersonalityGetResult {
        pub filename: String,
        #[serde(default)]
        pub content: Option<String>,
        pub exists: bool,
        #[serde(default)]
        pub truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub mtime_ms: Option<i64>,
    }
}

rpc_type! {
    pub struct PersonalityPutParams {
        pub agent: String,
        pub filename: String,
        pub content: String,
    }
}

rpc_type! {
    /// Consolidates gateway `PersonalityPutResponse`.
    pub struct PersonalityPutResult {
        pub bytes_written: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub mtime_ms: Option<i64>,
    }
}

rpc_type! {
    pub struct PersonalityTemplatesParams {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent: Option<String>,
    }
}

rpc_type! {
    /// Consolidates gateway `TemplateFile`.
    pub struct TemplateFileEntry {
        pub filename: String,
        pub content: String,
    }
}

rpc_type! {
    /// Consolidates gateway `TemplateResponse`.
    pub struct PersonalityTemplatesResult {
        pub preset: String,
        pub files: Vec<TemplateFileEntry>,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Config introspection (sections, catalog, status) ─────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    /// Consolidates gateway `CatalogModelProvider`.
    pub struct CatalogModelProvider {
        pub name: String,
        pub display_name: String,
        pub local: bool,
    }
}

rpc_type! {
    /// Consolidates gateway `CatalogResponse`.
    pub struct CatalogResponse {
        pub model_providers: Vec<CatalogModelProvider>,
    }
}

rpc_type! {
    pub struct CatalogModelsParams {
        /// Accepts `model_provider` or aliased `provider` (gateway compat).
        #[serde(alias = "provider")]
        pub model_provider: String,
    }
}

rpc_type! {
    /// Consolidates gateway `ModelsResponse`.
    pub struct CatalogModelsResult {
        pub model_provider: String,
        pub models: Vec<String>,
        pub local: bool,
        pub live: bool,
    }
}

rpc_type! {
    /// A config section entry for the dashboard sidebar / TUI section list.
    pub struct ConfigSectionEntry {
        pub key: String,
        pub label: String,
        pub help: String,
        pub has_picker: bool,
        pub completed: bool,
        /// Whether the section currently has enough usable config for the
        /// first-run path.
        #[serde(default)]
        pub ready: bool,
        /// Display group for the dashboard sidebar.
        #[serde(default)]
        pub group: String,
        /// `true` when this section is part of the canonical onboarding list.
        #[serde(default)]
        pub is_onboarding: bool,
        /// Editor shape (direct form / one-tier alias map / typed-family map /
        /// backend picker).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub shape: Option<zeroclaw_config::sections::SectionShape>,
    }
}

rpc_type! {
    /// Response for `config/sections`.
    pub struct ConfigSectionsResult {
        pub sections: Vec<ConfigSectionEntry>,
    }
}

rpc_type! {
    /// Config readiness status for the dashboard/TUI.
    pub struct ConfigStatusResult {
        pub needs_onboarding: bool,
        pub reason: String,
        pub has_partial_state: bool,
        pub missing: Vec<String>,
    }
}

rpc_type! {
    /// Consolidates gateway `PickerItem`.
    pub struct PickerItem {
        pub key: String,
        pub label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub badge: Option<String>,
    }
}

rpc_type! {
    /// Consolidates gateway `PickerResponse`.
    pub struct PickerResponse {
        pub section: String,
        pub items: Vec<PickerItem>,
        pub help: String,
    }
}

rpc_type! {
    pub struct SectionSelectParams {
        pub section: String,
        pub key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub alias: Option<String>,
    }
}

rpc_type! {
    /// Consolidates gateway `SelectItemResponse`.
    pub struct SelectItemResponse {
        pub fields_prefix: String,
        pub created: bool,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── File attachments ─────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Source hint for how the client obtained the file.
pub enum FileSource {
    Clipboard,
    #[default]
    File,
}

rpc_type! {
    /// A single file entry in a `file/attach` request. Either `path` (daemon
    /// reads from local disk — Unix socket only) or `data_b64` (client sends
    /// base64-encoded bytes) must be present.
    pub struct FileEntry {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub data_b64: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub mime_type: Option<String>,
        #[serde(default)]
        pub source: FileSource,
    }
}

rpc_type! {
    pub struct FileAttachParams {
        pub session_id: String,
        pub files: Vec<FileEntry>,
    }
}

rpc_type! {
    /// Result for a single file in a `file/attach` response.
    pub struct FileEntryResult {
        pub ref_id: String,
        pub marker: String,
        pub workspace_path: String,
        pub size_bytes: u64,
        pub deduplicated: bool,
    }
}

rpc_type! {
    pub struct FileAttachResult {
        pub files: Vec<FileEntryResult>,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Session approval ─────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    pub struct SessionApproveParams {
        pub session_id: String,
        pub request_id: String,
        pub decision: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub replacement: Option<String>,
    }
}

rpc_type! {
    pub struct SessionApproveResult {
        pub session_id: String,
        pub request_id: String,
        pub acknowledged: bool,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Logs ─────────────────────────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

rpc_type! {
    pub struct LogsSubscribeResult {
        pub subscribed: bool,
    }
}

rpc_type! {
    pub struct LogsQueryParams {
        #[serde(default)]
        pub since_ts: Option<String>,
        #[serde(default)]
        pub until_ts: Option<String>,
        #[serde(default)]
        pub until_id: Option<String>,
        #[serde(default)]
        pub severity_min: Option<u8>,
        #[serde(default)]
        pub q: Option<String>,
        #[serde(default)]
        pub category: Option<String>,
        #[serde(default)]
        pub action: Option<String>,
        #[serde(default)]
        pub outcome: Option<String>,
        #[serde(default)]
        pub trace_id: Option<String>,
        #[serde(default)]
        pub hide_internal: bool,
        #[serde(default)]
        pub limit: Option<usize>,
    }
}

rpc_type! {
    pub struct LogsQueryResult {
        pub events: Vec<serde_json::Value>,
        pub next_cursor: Option<(String, String)>,
        pub at_end: bool,
    }
}

// ══════════════════════════════════════════════════════════════════════
// ── Session update notifications ─────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

/// Typed session update events pushed via `session/update` notifications.
/// Replaces the hand-built `notification_for_turn_event` function.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionUpdateEvent {
    AgentMessageChunk {
        session_id: String,
        text: String,
    },
    AgentThoughtChunk {
        session_id: String,
        text: String,
    },
    ToolCall {
        session_id: String,
        tool_call_id: String,
        name: String,
        raw_input: Value,
    },
    ToolResult {
        session_id: String,
        tool_call_id: String,
        name: String,
        raw_output: String,
    },
    ApprovalRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        arguments_summary: String,
        timeout_secs: u64,
    },
}
