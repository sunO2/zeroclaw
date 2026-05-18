//! Capability handlers — Tauri commands the agent (or the dashboard webview)
//! can invoke to act on the local machine.
//!
//! Read-only ops (screenshot, read_ax) auto-approve.
//! Risky ops (click, type_keys, applescript) are gated behind a per-app approval
//! allowlist persisted via tauri-plugin-store.

pub mod applescript;
pub mod approval;
pub mod click;
pub mod notify;
pub mod read_ax;
pub mod screenshot;
pub mod type_keys;
