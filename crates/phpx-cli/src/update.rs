//! Update command - update project dependencies.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;

use phpx_pm::{
    ComposerBuilder,
    config::Config,
    installer::Installer,
    json::{ComposerJson, ComposerLock},
};

use crate::pm::platform::PlatformInfo;

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Packages to update (all if not specified)
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,

    /// Prefer source installation
    #[arg(long)]
    pub prefer_source: bool,

    /// Prefer dist installation
    #[arg(long)]
    pub prefer_dist: bool,

    /// Run in dry-run mode
    #[arg(long)]
    pub dry_run: bool,

    /// Skip dev dependencies
    #[arg(long)]
    pub no_dev: bool,

    /// Skip autoloader generation
    #[arg(long)]
    pub no_autoloader: bool,

    /// Skip script execution
    #[arg(long)]
    pub no_scripts: bool,

    /// Disable progress output
    #[arg(long)]
    pub no_progress: bool,

    /// Update also dependencies of the listed packages
    #[arg(short = 'w', long)]
    pub with_dependencies: bool,

    /// Update all dependencies including root requirements
    #[arg(short = 'W', long)]
    pub with_all_dependencies: bool,

    /// Prefer stable versions
    #[arg(long)]
    pub prefer_stable: bool,

    /// Prefer lowest versions (for testing)
    #[arg(long)]
    pub prefer_lowest: bool,

    /// Only update the lock file
    #[arg(long)]
    pub lock: bool,

    /// Optimize autoloader
    #[arg(short = 'o', long)]
    pub optimize_autoloader: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,

    // Common Composer flags (for compatibility)
    /// Force ANSI output
    #[arg(long)]
    pub ansi: bool,

    /// Disable ANSI output
    #[arg(long)]
    pub no_ansi: bool,

    /// Do not ask any interactive question
    #[arg(short = 'n', long)]
    pub no_interaction: bool,

    /// Do not output any message
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Skip the audit step after update (env: COMPOSER_NO_AUDIT)
    #[arg(long)]
    pub no_audit: bool,

    /// Audit output format (table, plain, json, or summary)
    #[arg(long, default_value = "summary")]
    pub audit_format: String,
}

pub async fn execute(args: UpdateArgs) -> Result<i32> {
    let skip_audit = args.no_audit || std::env::var("COMPOSER_NO_AUDIT").unwrap_or_default() == "1";

    // Initialize logger based on verbosity level
    // Only enable verbose logging for phpx crates, not dependencies
    let log_level = match args.verbose {
        0 => log::LevelFilter::Warn,
        1 => log::LevelFilter::Info,
        2 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("phpx_pm", log_level)
        .filter_module("phpx_cli", log_level)
        .format_timestamp(None)
        .format_target(false)
        .init();

    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    // Check for composer.json
    let json_path = working_dir.join("composer.json");
    if !json_path.exists() {
        eprintln!("{} No composer.json found in {}",
            style("Error:").red().bold(),
            working_dir.display()
        );
        return Ok(1);
    }

    // Parse composer.json
    let json_content = std::fs::read_to_string(&json_path)
        .context("Failed to read composer.json")?;
    let composer_json: ComposerJson = serde_json::from_str(&json_content)
        .context("Failed to parse composer.json")?;

    // Load composer.lock if it exists (to determine what's already installed)
    let lock_path = working_dir.join("composer.lock");
    let lock = if lock_path.exists() {
        let lock_content = std::fs::read_to_string(&lock_path)
            .context("Failed to read composer.lock")?;
        let lock: ComposerLock = serde_json::from_str(&lock_content)
            .context("Failed to parse composer.lock")?;
        Some(lock)
    } else {
        None
    };

    // Load config
    let config = Config::build(Some(&working_dir), true)?;

    // Detect platform
    let platform = PlatformInfo::detect();

    // Create Composer using builder
    let mut builder = ComposerBuilder::new(working_dir.clone())
        .with_config(config)
        .with_composer_json(composer_json)
        .with_composer_lock(lock)
        .with_platform_packages(platform.to_packages())
        .dry_run(args.dry_run)
        .no_dev(args.no_dev)
        .prefer_lowest(args.prefer_lowest);

    // Apply prefer_source/prefer_dist flags
    if args.prefer_source {
        builder = builder.prefer_source(true);
    } else if args.prefer_dist {
        builder = builder.prefer_dist(true);
    }

    let composer = builder.build()?;

    // Run Installer
    let installer = Installer::new(composer);

    let update_packages = if args.packages.is_empty() {
        None
    } else {
        Some(args.packages.clone())
    };

    let result = installer.update(
        args.optimize_autoloader,
        args.lock,
        update_packages,
    ).await;

    if result.is_ok() && !skip_audit {
        let audit_args = crate::pm::audit::AuditArgs {
            no_dev: args.no_dev,
            format: args.audit_format.clone(),
            locked: false,
            abandoned: Some("report".to_string()),
            working_dir: working_dir.clone(),
        };

        if let Err(e) = crate::pm::audit::execute(audit_args).await {
            eprintln!("Warning: Audit failed: {}", e);
        }
    }

    result
}
