//! Per-app approval allowlist for risky capability handlers.
//!
//! Per @m13v's review: Accessibility TCC is binary (the whole binary is
//! approved or denied), so "remember per-app" for risky ops (click, type_keys,
//! applescript) cannot map to OS state. Instead, approval decisions are
//! persisted via tauri-plugin-store in a JSON array keyed by capability name.
//!
//! Read-only ops (screenshot, read_ax) auto-approve and never touch this store.
//!
//! The approval prompt UI itself lives in the NodeClient dispatch path (#6321).
//! This module only provides the check/persist primitives that handlers and
//! the future dispatcher will call.

use serde::{Deserialize, Serialize};

const STORE_KEY: &str = "capability_approvals";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalEntry {
    pub capability: String,
    pub approved: bool,
}

/// Check whether a risky capability has been previously approved.
///
/// Returns `true` if the capability was previously approved and stored.
/// Returns `false` if not yet decided or explicitly denied.
#[cfg(target_os = "macos")]
pub fn is_approved(app: &tauri::AppHandle, capability: &str) -> bool {
    let entries = load_entries(app);
    entries
        .iter()
        .any(|e| e.capability == capability && e.approved)
}

/// Persist an approval decision for a capability.
#[cfg(target_os = "macos")]
pub fn set_approval(app: &tauri::AppHandle, capability: &str, approved: bool) {
    let mut entries = load_entries(app);
    entries.retain(|e| e.capability != capability);
    entries.push(ApprovalEntry {
        capability: capability.to_string(),
        approved,
    });
    save_entries(app, &entries);
}

/// Revoke all stored approvals (e.g. on TCC change).
#[cfg(target_os = "macos")]
pub fn clear_approvals(app: &tauri::AppHandle) {
    save_entries(app, &[]);
}

#[cfg(target_os = "macos")]
fn load_entries(app: &tauri::AppHandle) -> Vec<ApprovalEntry> {
    use tauri_plugin_store::StoreExt;
    let store = match app.store("settings.json") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let entries: Vec<ApprovalEntry> = store
        .get(STORE_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    entries
}

#[cfg(target_os = "macos")]
fn save_entries(app: &tauri::AppHandle, entries: &[ApprovalEntry]) {
    use tauri_plugin_store::StoreExt;
    let Ok(store) = app.store("settings.json") else {
        return;
    };
    let val = serde_json::to_value(entries).unwrap_or_default();
    store.set(STORE_KEY, val);
    let _ = store.save();
}

/// Whether a capability is considered "risky" and requires approval gating.
pub fn is_risky(capability: &str) -> bool {
    matches!(capability, "click" | "type_keys" | "applescript")
}
