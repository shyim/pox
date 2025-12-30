//! Install command - install project dependencies.

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

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Prefer source installation (git clone)
    #[arg(long)]
    pub prefer_source: bool,

    /// Prefer dist installation (zip download)
    #[arg(long)]
    pub prefer_dist: bool,

    /// Run in dry-run mode (no actual changes)
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

    /// Optimize autoloader (convert PSR-4/PSR-0 to classmap)
    #[arg(short = 'o', long)]
    pub optimize_autoloader: bool,

    /// Use authoritative classmap (only load from classmap)
    #[arg(short = 'a', long)]
    pub classmap_authoritative: bool,

    /// Use APCu to cache found/not-found classes
    #[arg(long)]
    pub apcu_autoloader: bool,

    /// Ignore platform requirements
    #[arg(long)]
    pub ignore_platform_reqs: bool,

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

    /// Skip the audit step after installation (env: COMPOSER_NO_AUDIT)
    #[arg(long)]
    pub no_audit: bool,

    /// Audit output format (table, plain, json, or summary)
    #[arg(long, default_value = "summary")]
    pub audit_format: String,
}

use crate::pm::platform::PlatformInfo;

pub async fn execute(args: InstallArgs) -> Result<i32> {
    let skip_audit = args.no_audit || std::env::var("COMPOSER_NO_AUDIT").unwrap_or_default() == "1";

    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    // Load composer.json
    let json_path = working_dir.join("composer.json");
    let composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
        ComposerJson::default()
    };

    // Check for composer.lock
    let lock_path = working_dir.join("composer.lock");
    let (lock, run_update) = if lock_path.exists() {
        let lock_content = std::fs::read_to_string(&lock_path)
            .context("Failed to read composer.lock")?;
        let lock: ComposerLock = serde_json::from_str(&lock_content)
            .context("Failed to parse composer.lock")?;
        (Some(lock), false)
    } else {
        println!("{} No composer.lock file found. Running update to generate one.", style("Info:").cyan());
        (None, true)
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
        .no_dev(args.no_dev);

    // Apply prefer_source/prefer_dist flags
    if args.prefer_source {
        builder = builder.prefer_source(true);
    } else if args.prefer_dist {
        builder = builder.prefer_dist(true);
    }

    let composer = builder.build()?;

    // Run Installer
    let installer = Installer::new(composer);

    let result = if run_update {
        installer.update(
            args.optimize_autoloader,
            false,
            None,
        ).await
    } else {
        installer.install(
            args.no_scripts,
            args.optimize_autoloader,
            args.classmap_authoritative,
            args.apcu_autoloader,
            args.ignore_platform_reqs
        ).await
    };

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
