//! Install command - install project dependencies.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;

use phpx_pm::{
    Composer,
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
}

use crate::pm::platform::PlatformInfo;

pub async fn execute(args: InstallArgs) -> Result<i32> {
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

    // Create Composer
    let composer = Composer::new(
        working_dir.clone(),
        config,
        composer_json,
        lock
    )?;

    // Run Installer
    let installer = Installer::new(composer);

    if run_update {
        // Detect platform for update
        println!("Detecting platform...");
        let platform = PlatformInfo::detect();
        let platform_packages = platform.to_packages();

        installer.update(
            platform_packages,
            args.dry_run,
            args.no_dev,
            args.optimize_autoloader,
            false, // prefer_lowest
            false  // lock (update_lock_only)
        ).await
    } else {
        installer.install(
            args.dry_run,
            args.no_dev,
            args.no_scripts,
            args.optimize_autoloader,
            args.classmap_authoritative,
            args.apcu_autoloader,
            args.ignore_platform_reqs
        ).await
    }
}
