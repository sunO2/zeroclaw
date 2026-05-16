//! CLI handler for the `zeroclaw dream` command.

use anyhow::{Context, Result};
use zeroclaw_config::schema::Config;
use zeroclaw_runtime::dream::report::DreamReport;

/// Run a manual dream cycle from the CLI.
pub async fn run_dream(config: &Config, dry_run: bool, verbose: bool) -> Result<()> {
    use zeroclaw_runtime::dream::engine::DreamEngine;

    // Build dream config with audit_mode = dry_run.
    let mut dream_config = config.dream_mode.clone();
    if dry_run {
        dream_config.audit_mode = true;
    }

    let engine = DreamEngine::new(dream_config, config.workspace_dir.clone());

    // Resolve provider.
    let fallback = config
        .providers
        .fallback_provider()
        .context("dream: no fallback provider configured")?;
    let provider_name = config.providers.fallback.as_deref().unwrap_or("anthropic");
    let provider = zeroclaw_providers::create_provider(provider_name, fallback.api_key.as_deref())?;
    let model = config
        .dream_mode
        .model
        .as_deref()
        .or(fallback.model.as_deref())
        .unwrap_or("claude-haiku-4-5-20251001");

    // Create memory backend.
    let memory = zeroclaw_memory::create_memory(
        &config.memory,
        &config.workspace_dir,
        config
            .providers
            .fallback_provider()
            .and_then(|e| e.api_key.as_deref()),
    )
    .context("dream: failed to create memory backend")?;

    if verbose {
        println!("Dream cycle starting...");
        println!("  Provider: {provider_name}");
        println!("  Model: {model}");
        println!("  Memory backend: {}", memory.name());
        if dry_run {
            println!("  Mode: dry-run (no changes will be persisted)");
        }
    }

    let result = engine
        .run_cycle(memory.as_ref(), provider.as_ref(), model)
        .await?;

    println!(
        "Dream cycle complete: {} memories gathered, {} insights consolidated, {} pruned",
        result.gathered_count, result.consolidated_count, result.pruned_count
    );

    if !result.insights.is_empty() {
        println!("\nInsights:");
        for (i, insight) in result.insights.iter().enumerate() {
            println!("  {}. {insight}", i + 1);
        }
    }

    if let Some(ref summary) = result.report_summary {
        println!("\nSummary: {summary}");
    }

    if dry_run {
        println!("\n[dry-run] No changes were persisted to memory.");
    }

    Ok(())
}

/// Show the pending dream report, if any.
pub fn show_report(config: &Config) -> Result<()> {
    match DreamReport::load_pending(&config.workspace_dir)? {
        Some(report) => {
            println!("{}", report.format_message());
            DreamReport::mark_delivered(&config.workspace_dir)?;
        }
        None => {
            println!("No pending dream report.");
        }
    }
    Ok(())
}
