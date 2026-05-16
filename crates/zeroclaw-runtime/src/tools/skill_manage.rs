//! Skill management tool: lets the agent patch (improve) skills at runtime.
//!
//! The `patch` action wraps [`SkillImprover::improve_skill`] and enforces
//! cooldown logic, returning distinct error messages for "feature disabled"
//! vs "cooldown active".

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use zeroclaw_api::tool::{Tool, ToolResult};
use zeroclaw_config::schema::SkillImprovementConfig;

use crate::skills::improver::SkillImprover;

/// Tool that exposes skill management actions to the agent.
pub struct SkillManageTool {
    improver: Arc<Mutex<SkillImprover>>,
    config: SkillImprovementConfig,
}

impl SkillManageTool {
    pub fn new(workspace_dir: PathBuf, config: SkillImprovementConfig) -> Self {
        let improver = SkillImprover::new(workspace_dir, config.clone());
        Self {
            improver: Arc::new(Mutex::new(improver)),
            config,
        }
    }

    /// Execute the `patch` action: improve an existing skill file.
    async fn handle_patch(&self, args: &serde_json::Value) -> anyhow::Result<ToolResult> {
        let slug = args
            .get("slug")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'slug' parameter for patch action"))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter for patch action"))?;

        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("agent-initiated improvement");

        // Check disabled vs cooldown before calling improve_skill, so we can
        // return distinct error messages.
        if !self.config.enabled {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Skill improvement is disabled (enabled: false)".to_string()),
            });
        }

        // Check cooldown separately.
        let cooldown_blocked = {
            let improver = self.improver.lock().await;
            !improver.should_improve_skill(slug)
        };

        if cooldown_blocked {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Skill '{slug}' is in cooldown; try again later")),
            });
        }

        // Perform the improvement.
        let result = {
            let mut improver = self.improver.lock().await;
            improver.improve_skill(slug, content, reason).await
        };

        match result {
            Ok(Some(improved_slug)) => Ok(ToolResult {
                success: true,
                output: format!("Skill '{improved_slug}' patched successfully."),
                error: None,
            }),
            Ok(None) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Skill '{slug}' was not improved (skipped).")),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to patch skill '{slug}': {e}")),
            }),
        }
    }
}

#[async_trait]
impl Tool for SkillManageTool {
    fn name(&self) -> &str {
        "skill_manage"
    }

    fn description(&self) -> &str {
        "Manage skills at runtime. Actions: patch (improve an existing skill file)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["patch"],
                    "description": "Action to perform."
                },
                "slug": {
                    "type": "string",
                    "description": "Skill slug (directory name under skills/)."
                },
                "content": {
                    "type": "string",
                    "description": "New TOML content for the skill file."
                },
                "reason": {
                    "type": "string",
                    "description": "Reason for the improvement (recorded in audit trail)."
                }
            },
            "required": ["action", "slug"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;

        match action {
            "patch" => self.handle_patch(&args).await,
            other => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Unknown action '{other}'. Use: patch.")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn skill_manage_patch_blocked_when_improvement_disabled() {
        let tool = SkillManageTool::new(
            PathBuf::from("/tmp/test"),
            SkillImprovementConfig {
                enabled: false,
                cooldown_secs: 0,
            },
        );

        let args = json!({
            "action": "patch",
            "slug": "my-skill",
            "content": "[skill]\nname = \"my-skill\"\n",
            "reason": "test"
        });

        let result = tool.execute(args).await.unwrap();

        assert!(!result.success);
        assert_eq!(
            result.error.as_deref(),
            Some("Skill improvement is disabled (enabled: false)")
        );
    }

    #[tokio::test]
    async fn skill_manage_patch_blocked_by_cooldown() {
        let tool = SkillManageTool::new(
            PathBuf::from("/tmp/test"),
            SkillImprovementConfig {
                enabled: true,
                cooldown_secs: 9999,
            },
        );

        // Insert a cooldown entry.
        {
            let mut improver = tool.improver.lock().await;
            improver
                .cooldowns
                .insert("my-skill".to_string(), Instant::now());
        }

        let args = json!({
            "action": "patch",
            "slug": "my-skill",
            "content": "[skill]\nname = \"my-skill\"\n",
            "reason": "test"
        });

        let result = tool.execute(args).await.unwrap();

        assert!(!result.success);
        assert_eq!(
            result.error.as_deref(),
            Some("Skill 'my-skill' is in cooldown; try again later")
        );
    }

    #[tokio::test]
    async fn skill_manage_patch_success() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("test-skill");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.toml"),
            "[skill]\nname = \"test-skill\"\ndescription = \"Original\"\nversion = \"0.1.0\"\n",
        )
        .await
        .unwrap();

        let tool = SkillManageTool::new(
            dir.path().to_path_buf(),
            SkillImprovementConfig {
                enabled: true,
                cooldown_secs: 0,
            },
        );

        let args = json!({
            "action": "patch",
            "slug": "test-skill",
            "content": "[skill]\nname = \"test-skill\"\ndescription = \"Improved\"\nversion = \"0.2.0\"\n",
            "reason": "better description"
        });

        let result = tool.execute(args).await.unwrap();
        assert!(result.success, "expected success, got: {:?}", result.error);
        assert!(result.output.contains("patched successfully"));
    }

    #[tokio::test]
    async fn skill_manage_unknown_action() {
        let tool = SkillManageTool::new(
            PathBuf::from("/tmp/test"),
            SkillImprovementConfig {
                enabled: true,
                cooldown_secs: 0,
            },
        );

        let args = json!({ "action": "delete", "slug": "x" });
        let result = tool.execute(args).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("Unknown action"));
    }
}
