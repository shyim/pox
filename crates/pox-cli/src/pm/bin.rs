//! Bin command - run composer commands in isolated vendor-bin namespaces.
//!
//! This is a native Rust port of bamarni/composer-bin-plugin.
//! It allows running composer commands in isolated directories under vendor-bin/,
//! enabling separate dependency trees for development tools.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;
use std::process::Command;

/// Configuration for the bin plugin from composer.json extra.bamarni-bin
#[derive(Debug, Clone)]
pub struct BinConfig {
    /// Whether to create bin links in the main vendor/bin directory
    pub bin_links: bool,
    /// Target directory for bin namespaces (default: vendor-bin)
    pub target_directory: String,
    /// Whether to forward install/update commands to all namespaces
    pub forward_command: bool,
}

impl Default for BinConfig {
    fn default() -> Self {
        Self {
            bin_links: false,  // Default to false in 2.x behavior
            target_directory: "vendor-bin".to_string(),
            forward_command: false,
        }
    }
}

impl BinConfig {
    /// Parse config from composer.json extra field
    pub fn from_extra(extra: &serde_json::Value) -> Self {
        let bamarni_bin = extra.get("bamarni-bin");

        let mut config = Self::default();

        if let Some(obj) = bamarni_bin.and_then(|v| v.as_object()) {
            if let Some(bin_links) = obj.get("bin-links").and_then(|v| v.as_bool()) {
                config.bin_links = bin_links;
            }
            if let Some(target_dir) = obj.get("target-directory").and_then(|v| v.as_str()) {
                config.target_directory = target_dir.to_string();
            }
            if let Some(forward) = obj.get("forward-command").and_then(|v| v.as_bool()) {
                config.forward_command = forward;
            }
        }

        config
    }
}

#[derive(Args, Debug)]
#[command(trailing_var_arg = true)]
pub struct BinArgs {
    /// Working directory
    #[arg(short = 'd', long, default_value = ".", global = true)]
    pub working_dir: PathBuf,

    /// Bin namespace (e.g., 'php-cs-fixer', 'phpstan', or 'all' for all namespaces)
    #[arg(value_name = "NAMESPACE")]
    pub namespace: String,

    /// Command and arguments to pass to composer (supports flags like --ansi)
    #[arg(value_name = "ARGS", num_args = 0..)]
    pub args: Vec<String>,
}

pub async fn execute(args: BinArgs) -> Result<i32> {
    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    // Load composer.json to get config
    let composer_json_path = working_dir.join("composer.json");
    let config = if composer_json_path.exists() {
        let content = std::fs::read_to_string(&composer_json_path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;
        BinConfig::from_extra(json.get("extra").unwrap_or(&serde_json::Value::Null))
    } else {
        BinConfig::default()
    };

    let vendor_bin_root = working_dir.join(&config.target_directory);

    if args.namespace == "all" {
        execute_all_namespaces(&working_dir, &vendor_bin_root, &args.args, &config).await
    } else {
        execute_in_namespace(&working_dir, &vendor_bin_root, &args.namespace, &args.args, &config).await
    }
}

/// Execute command in all bin namespaces
async fn execute_all_namespaces(
    _working_dir: &PathBuf,
    vendor_bin_root: &PathBuf,
    command_args: &[String],
    config: &BinConfig,
) -> Result<i32> {
    if !vendor_bin_root.exists() {
        println!("{} No bin namespaces found in {}",
            style("Warning:").yellow().bold(),
            config.target_directory
        );
        return Ok(0);
    }

    let namespaces: Vec<_> = std::fs::read_dir(vendor_bin_root)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    if namespaces.is_empty() {
        println!("{} No bin namespaces found.",
            style("Warning:").yellow().bold()
        );
        return Ok(0);
    }

    let mut total_exit_code = 0;

    for entry in namespaces {
        let namespace_name = entry.file_name().to_string_lossy().to_string();
        let namespace_dir = entry.path();

        let exit_code = run_in_namespace(&namespace_dir, &namespace_name, command_args, config)?;
        total_exit_code += exit_code;
    }

    Ok(total_exit_code.min(255))
}

/// Execute command in a specific bin namespace
async fn execute_in_namespace(
    _working_dir: &PathBuf,
    vendor_bin_root: &PathBuf,
    namespace: &str,
    command_args: &[String],
    config: &BinConfig,
) -> Result<i32> {
    let namespace_dir = vendor_bin_root.join(namespace);

    // Create namespace directory if it doesn't exist
    if !namespace_dir.exists() {
        std::fs::create_dir_all(&namespace_dir)
            .context(format!("Failed to create namespace directory: {}", namespace_dir.display()))?;
    }

    // Ensure composer.json exists in namespace
    let namespace_composer = namespace_dir.join("composer.json");
    if !namespace_composer.exists() {
        std::fs::write(&namespace_composer, "{}")
            .context("Failed to create composer.json in namespace")?;
    }

    run_in_namespace(&namespace_dir, namespace, command_args, config)
}

/// Run phpx pm command in a namespace directory
fn run_in_namespace(
    namespace_dir: &PathBuf,
    namespace_name: &str,
    command_args: &[String],
    _config: &BinConfig,
) -> Result<i32> {
    println!("{} Running in namespace {}",
        style("Bin:").cyan().bold(),
        style(namespace_name).yellow()
    );

    if command_args.is_empty() {
        println!("{} No command specified. Usage: phpx pm bin {} <command>",
            style("Error:").red().bold(),
            namespace_name
        );
        return Ok(1);
    }

    // Get the current executable path
    let current_exe = std::env::current_exe()
        .context("Failed to get current executable path")?;

    // Build the command: phpx <command> -d <namespace_dir>
    // The first arg is the composer command (install, update, require, etc.)
    let composer_command = &command_args[0];
    let rest_args = &command_args[1..];

    let mut cmd = Command::new(&current_exe);
    cmd.arg(composer_command);
    cmd.arg("-d").arg(namespace_dir);
    cmd.args(rest_args);

    // Inherit stdio for interactive output
    cmd.stdin(std::process::Stdio::inherit());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());

    let status = cmd.status()
        .context(format!("Failed to execute command in namespace {}", namespace_name))?;

    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bin_config_default() {
        let config = BinConfig::default();
        assert!(!config.bin_links);
        assert_eq!(config.target_directory, "vendor-bin");
        assert!(!config.forward_command);
    }

    #[test]
    fn test_bin_config_from_extra() {
        let extra = serde_json::json!({
            "bamarni-bin": {
                "bin-links": true,
                "target-directory": "tools",
                "forward-command": true
            }
        });

        let config = BinConfig::from_extra(&extra);
        assert!(config.bin_links);
        assert_eq!(config.target_directory, "tools");
        assert!(config.forward_command);
    }

    #[test]
    fn test_bin_config_from_empty_extra() {
        let extra = serde_json::json!({});
        let config = BinConfig::from_extra(&extra);
        assert!(!config.bin_links);
        assert_eq!(config.target_directory, "vendor-bin");
        assert!(!config.forward_command);
    }
}
