//! Run command - execute scripts defined in composer.json.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;

use phpx_pm::json::ComposerJson;

use phpx_pm::scripts;

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Script name to run
    #[arg(value_name = "SCRIPT")]
    pub script: Option<String>,

    /// List available scripts
    #[arg(short = 'l', long)]
    pub list: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,

    /// Arguments passed to the script
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn execute(args: RunArgs) -> Result<i32> {
    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    // Load composer.json
    let json_path = working_dir.join("composer.json");
    if !json_path.exists() {
        eprintln!("{} No composer.json found in {}",
            style("Error:").red().bold(),
            working_dir.display()
        );
        return Ok(1);
    }

    let content = std::fs::read_to_string(&json_path)?;
    let composer_json: ComposerJson = serde_json::from_str(&content)?;

    // If --list or no script specified, show available scripts
    if args.list || args.script.is_none() {
        return scripts::list_scripts(&composer_json);
    }

    let script_name = args.script.as_ref().unwrap();

    // Run the script
    scripts::run_script(script_name, &composer_json, &working_dir, &args.args)
}
