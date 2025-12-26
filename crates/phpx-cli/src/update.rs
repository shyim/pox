//! Update command - update project dependencies.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;

use phpx_pm::{
    Composer,
    config::Config,
    installer::Installer,
    json::ComposerJson,
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
}

pub async fn execute(args: UpdateArgs) -> Result<i32> {
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

    // Load config
    let config = Config::build(Some(&working_dir), true)?;

    // Create Composer
    let composer = Composer::new(
        working_dir.clone(),
        config,
        composer_json,
        None
    )?;

    // Detect platform
    println!("Detecting platform...");
    let platform = PlatformInfo::detect();
    let platform_packages = platform.to_packages();

    // Run Installer
    let installer = Installer::new(composer);
    
    // Pass relevant args to update
    installer.update(
        platform_packages,
        args.dry_run,
        args.no_dev,
        args.optimize_autoloader,
        args.prefer_lowest,
        args.lock
    ).await
}
