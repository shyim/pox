//! Dump-autoload command - regenerate the autoloader.

use anyhow::{Context, Result};
use clap::Args;
use std::path::PathBuf;

use pox_pm::{
    ComposerBuilder,
    config::Config,
    installer::Installer,
    json::{ComposerJson, ComposerLock},
};

#[derive(Args, Debug)]
pub struct DumpAutoloadArgs {
    /// Optimize autoloader (convert PSR-4/PSR-0 to classmap)
    #[arg(short = 'o', long)]
    pub optimize: bool,

    /// Use authoritative classmap (only load from classmap)
    #[arg(short = 'a', long)]
    pub classmap_authoritative: bool,

    /// Use APCu to cache found/not-found classes
    #[arg(long)]
    pub apcu: bool,

    /// Skip dev dependencies
    #[arg(long)]
    pub no_dev: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

pub async fn execute(args: DumpAutoloadArgs) -> Result<i32> {
    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    // Load composer.json
    let json_path = working_dir.join("composer.json");
    let composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
         ComposerJson::default() // Or handle strictly? dump-autoload usually works with at least a json or installed.
         // If generic default, it's fine.
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

    // Create Composer using builder
    let composer = ComposerBuilder::new(working_dir.clone())
        .with_config(config)
        .with_composer_json(composer_json)
        .with_composer_lock(lock)
        .no_dev(args.no_dev)
        .build()?;

    // Run Installer
    let installer = Installer::new(composer);
    
    installer.dump_autoload(
        args.optimize,
        args.classmap_authoritative,
        args.apcu,
        args.no_dev,
    )?;

    Ok(0)
}
