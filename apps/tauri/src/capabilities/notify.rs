//! Notification capability — posts a system notification via osascript
//! `display notification`. Requires the Notifications TCC permission to
//! surface the banner. Uses osascript directly (no extra crate) to keep
//! dependencies minimal.

use std::process::Command;

/// Post a system notification with title, optional body, and optional subtitle.
#[tauri::command]
pub fn notify(title: String, body: Option<String>, subtitle: Option<String>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::macos::permissions;
        if permissions::check_notifications() != "granted" {
            return Err("permission_denied(notifications)".into());
        }

        let mut script = format!(
            "display notification {quote}{body}{quote}",
            quote = "\"",
            body = body.as_deref().unwrap_or("")
        );
        script = format!(
            "{script} with title {quote}{title}{quote}",
            quote = "\"",
            title = title
        );
        if let Some(ref sub) = subtitle {
            script = format!(
                "{script} subtitle {quote}{sub}{quote}",
                quote = "\"",
                sub = sub
            );
        }

        let output = Command::new("/usr/bin/osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("osascript spawn failed: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("osascript exited with {}", output.status)
            } else {
                stderr
            });
        }

        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (title, body, subtitle);
        Err("Notification capability is currently macOS-only".into())
    }
}
