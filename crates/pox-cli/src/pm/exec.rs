//! Exec command - execute a vendored binary/script.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;

#[derive(Args, Debug)]
pub struct ExecArgs {
    /// Binary name to execute
    #[arg(value_name = "BINARY")]
    pub binary: Option<String>,

    /// List available binaries
    #[arg(short = 'l', long)]
    pub list: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,

    /// Arguments passed to the binary
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn execute(args: ExecArgs) -> Result<i32> {
    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    let vendor_bin = working_dir.join("vendor/bin");

    let binaries = get_available_binaries(&vendor_bin)?;

    if args.list {
        return list_binaries(&binaries, &vendor_bin);
    }

    let binary_name = if let Some(name) = args.binary.as_ref() {
        name.clone()
    } else {
        if binaries.is_empty() {
            if !vendor_bin.exists() {
                println!("{} No vendor/bin directory found. Run 'pox install' first.",
                    style("Info:").cyan()
                );
            } else {
                println!("{} No binaries found in vendor/bin",
                    style("Info:").cyan()
                );
            }
            return Ok(0);
        }

        if !std::io::stdout().is_terminal() {
            return list_binaries(&binaries, &vendor_bin);
        }

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select a binary to run")
            .items(&binaries)
            .default(0)
            .interact_opt()
            .context("Failed to show selection prompt")?;

        match selection {
            Some(idx) => binaries[idx].clone(),
            None => return Ok(0),
        }
    };

    let binary_path = find_binary(&vendor_bin, &binary_name)?;

    match binary_path {
        Some(path) => execute_binary(&path, &args.args, &working_dir),
        None => {
            eprintln!("{} Binary '{}' not found in vendor/bin",
                style("Error:").red().bold(),
                binary_name
            );

            if !binaries.is_empty() {
                eprintln!();
                eprintln!("Available binaries:");
                for bin in &binaries {
                    eprintln!("  - {}", bin);
                }
            } else {
                eprintln!();
                eprintln!("No binaries found. Run 'pox install' first.");
            }

            Ok(1)
        }
    }
}

/// Get list of available binaries in vendor/bin
fn get_available_binaries(vendor_bin: &PathBuf) -> Result<Vec<String>> {
    let mut binaries = Vec::new();

    if !vendor_bin.exists() {
        return Ok(binaries);
    }

    let entries = std::fs::read_dir(vendor_bin)
        .context("Failed to read vendor/bin directory")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            continue;
        }

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            #[cfg(unix)]
            if name.ends_with(".bat") {
                continue;
            }

            #[cfg(windows)]
            {
                let base_name = name.strip_suffix(".bat").unwrap_or(name);
                if name.ends_with(".bat") && binaries.contains(&base_name.to_string()) {
                    continue;
                }
            }

            binaries.push(name.to_string());
        }
    }

    binaries.sort();
    Ok(binaries)
}

/// Find a binary by name
fn find_binary(vendor_bin: &PathBuf, name: &str) -> Result<Option<PathBuf>> {
    if !vendor_bin.exists() {
        return Ok(None);
    }

    let exact_path = vendor_bin.join(name);
    if exact_path.exists() && exact_path.is_file() {
        return Ok(Some(exact_path));
    }

    #[cfg(windows)]
    {
        let bat_path = vendor_bin.join(format!("{}.bat", name));
        if bat_path.exists() && bat_path.is_file() {
            return Ok(Some(bat_path));
        }
    }

    let entries = std::fs::read_dir(vendor_bin)?;
    let name_lower = name.to_lowercase();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                let file_name_lower = file_name.to_lowercase();

                #[cfg(windows)]
                let file_name_lower = file_name_lower
                    .strip_suffix(".bat")
                    .unwrap_or(&file_name_lower)
                    .to_string();

                if file_name_lower == name_lower {
                    return Ok(Some(path));
                }
            }
        }
    }

    Ok(None)
}

/// List available binaries
fn list_binaries(binaries: &[String], vendor_bin: &PathBuf) -> Result<i32> {
    if binaries.is_empty() {
        if !vendor_bin.exists() {
            println!("{} No vendor/bin directory found. Run 'pox install' first.",
                style("Info:").cyan()
            );
        } else {
            println!("{} No binaries found in vendor/bin",
                style("Info:").cyan()
            );
        }
        return Ok(0);
    }

    println!("{} Available binaries:\n", style("Exec:").cyan().bold());

    for binary in binaries {
        println!("  {} {}", style("-").dim(), style(binary).green());
    }

    println!();
    println!("{} Run with: {} <binary> [args...]",
        style("Usage:").dim(),
        style("pox pm exec").cyan()
    );

    Ok(0)
}

/// Execute a binary with arguments
fn execute_binary(path: &PathBuf, args: &[String], working_dir: &PathBuf) -> Result<i32> {
    let is_php_script = is_php_file(path)?;

    let status = if is_php_script {
        let pox_binary = std::env::current_exe()
            .context("Failed to get current executable path")?;

        Command::new(&pox_binary)
            .arg(path)
            .args(args)
            .current_dir(working_dir)
            .status()
            .with_context(|| format!("Failed to execute {}", path.display()))?
    } else {
        #[cfg(unix)]
        {
            Command::new(path)
                .args(args)
                .current_dir(working_dir)
                .status()
                .with_context(|| format!("Failed to execute {}", path.display()))?
        }

        #[cfg(windows)]
        {
            Command::new("cmd")
                .arg("/C")
                .arg(path)
                .args(args)
                .current_dir(working_dir)
                .status()
                .with_context(|| format!("Failed to execute {}", path.display()))?
        }
    };

    Ok(status.code().unwrap_or(1))
}

fn is_php_file(path: &PathBuf) -> Result<bool> {
    if let Some(ext) = path.extension() {
        if ext == "php" {
            return Ok(true);
        }
    }

    let content = std::fs::read(path)
        .context("Failed to read binary file")?;

    if content.starts_with(b"#!") {
        if let Some(pos) = content.iter().position(|&b| b == b'\n') {
            let first_line = String::from_utf8_lossy(&content[..pos]);
            if first_line.contains("php") {
                return Ok(true);
            }
        }
    }

    if content.starts_with(b"<?php") || content.starts_with(b"<?PHP") {
        return Ok(true);
    }

    if content.len() > 8 && content.starts_with(&[0xEF, 0xBB, 0xBF]) {
        if content[3..].starts_with(b"<?php") || content[3..].starts_with(b"<?PHP") {
            return Ok(true);
        }
    }

    Ok(false)
}
