//! Add command - add and install a package.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;

use pox_pm::{
    ComposerBuilder,
    config::Config,
    installer::Installer,
    json::{ComposerJson, ComposerLock},
};
use crate::pm::platform::PlatformInfo;

#[derive(Args, Debug)]
pub struct AddArgs {
    /// Packages to require (e.g., vendor/package:^1.0)
    #[arg(value_name = "PACKAGES", required = true)]
    pub packages: Vec<String>,

    /// Add as development dependency
    #[arg(long)]
    pub dev: bool,

    /// Prefer source installation
    #[arg(long)]
    pub prefer_source: bool,

    /// Prefer dist installation
    #[arg(long)]
    pub prefer_dist: bool,

    /// Run in dry-run mode
    #[arg(long)]
    pub dry_run: bool,

    /// Skip autoloader generation
    #[arg(long)]
    pub no_autoloader: bool,

    /// Skip script execution
    #[arg(long)]
    pub no_scripts: bool,

    /// Do not run update after adding
    #[arg(long)]
    pub no_update: bool,

    /// Optimize autoloader
    #[arg(short = 'o', long)]
    pub optimize_autoloader: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

pub async fn execute(args: AddArgs) -> Result<i32> {
    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    // Load composer.json
    let json_path = working_dir.join("composer.json");
    let composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
        println!("{} No composer.json found. Creating one.", style("Info:").cyan());
        ComposerJson::default()
    };

    // Load composer.lock
    let lock_path = working_dir.join("composer.lock");
    let lock: Option<ComposerLock> = if lock_path.exists() {
        let content = std::fs::read_to_string(&lock_path)
            .context("Failed to read composer.lock")?;
        serde_json::from_str(&content).ok()
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
        .dry_run(args.dry_run);

    // Apply prefer_source/prefer_dist flags
    if args.prefer_source {
        builder = builder.prefer_source(true);
    } else if args.prefer_dist {
        builder = builder.prefer_dist(true);
    }

    let mut composer = builder.build()?;

    println!("{} Adding packages", style("Composer").green().bold());
    if args.dry_run {
        println!("{} Running in dry-run mode", style("Info:").cyan());
    }

    // Modify composer.json (in-memory)
    for spec in &args.packages {
        let (name, constraint) = parse_package_spec(spec);

        println!("  {} {} {}",
            style("+").green(),
            style(&name).white().bold(),
            style(&constraint).yellow()
        );

        if args.dev {
            composer.composer_json.require_dev.insert(name, constraint);
        } else {
            composer.composer_json.require.insert(name, constraint);
        }
    }

    // Write updated composer.json
    if !args.dry_run {
        let content = serde_json::to_string_pretty(&composer.composer_json)
            .context("Failed to serialize composer.json")?;
        std::fs::write(&json_path, content)
            .context("Failed to write composer.json")?;
    }

    // Run update
    if !args.no_update {
        // Run Installer
        let installer = Installer::new(composer);

        let new_packages: Vec<String> = args.packages.iter()
            .map(|spec| parse_package_spec(spec).0)
            .collect();

        installer.update(
            args.optimize_autoloader,
            false,
            Some(new_packages),
        ).await
    } else {
        println!("{} Packages added to composer.json", style("Success:").green().bold());
        Ok(0)
    }
}

/// Parse a package specification (vendor/package:^1.0 or vendor/package)
fn parse_package_spec(spec: &str) -> (String, String) {
    if let Some(pos) = spec.find(':') {
        let name = spec[..pos].to_string();
        let constraint = spec[pos + 1..].to_string();
        (name, constraint)
    } else {
        // Default to any version
        (spec.to_string(), "*".to_string())
    }
}
