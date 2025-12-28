//! Show command - display package information.

use anyhow::{Context, Result};
use clap::Args;
use std::path::PathBuf;
use std::sync::Arc;

use phpx_pm::{
    Repository,
    config::Config,
    json::{ComposerJson, ComposerLock},
    is_platform_package,
};
use std::collections::HashSet;

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Package to inspect (or wildcard pattern)
    pub package: Option<String>,

    /// Version or version constraint to inspect
    pub version: Option<String>,

    /// List all packages
    #[arg(long)]
    pub all: bool,

    /// List all locked packages
    #[arg(long)]
    pub locked: bool,

    /// List platform packages only
    #[arg(short = 'p', long)]
    pub platform: bool,

    /// List available packages only
    #[arg(short = 'a', long)]
    pub available: bool,

    /// Show the root package information
    #[arg(short = 's', long = "self")]
    pub self_package: bool,

    /// List package names only
    #[arg(short = 'N', long)]
    pub name_only: bool,

    /// Show package paths
    #[arg(short = 'P', long)]
    pub path: bool,

    /// List dependencies as a tree
    #[arg(short = 't', long)]
    pub tree: bool,

    /// Show the latest version
    #[arg(short = 'l', long)]
    pub latest: bool,

    /// Show only outdated packages
    #[arg(short = 'o', long)]
    pub outdated: bool,

    /// Show only packages directly required by root
    #[arg(short = 'D', long)]
    pub direct: bool,

    /// Output format: text or json
    #[arg(short = 'f', long, default_value = "text")]
    pub format: String,

    /// Disables search in require-dev packages
    #[arg(long)]
    pub no_dev: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

pub async fn execute(args: ShowArgs) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

    if args.format != "text" && args.format != "json" {
        eprintln!("Error: Unsupported format '{}'. Use 'text' or 'json'.", args.format);
        return Ok(1);
    }

    if args.direct && (args.all || args.available || args.platform) {
        eprintln!("Error: --direct is not usable with --all, --platform, or --available");
        return Ok(1);
    }

    if args.tree && (args.all || args.available) {
        eprintln!("Error: --tree is not usable with --all or --available");
        return Ok(1);
    }

    if args.tree && args.latest {
        eprintln!("Error: --tree is not usable with --latest");
        return Ok(1);
    }

    if args.tree && args.path {
        eprintln!("Error: --tree is not usable with --path");
        return Ok(1);
    }

    if args.outdated {
        // --outdated implies --latest
    }

    let json_path = working_dir.join("composer.json");
    let composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
        ComposerJson::default()
    };

    let lock: Option<ComposerLock> = {
        let lock_path = working_dir.join("composer.lock");
        if lock_path.exists() {
            let content = std::fs::read_to_string(&lock_path).ok();
            content.and_then(|c| serde_json::from_str(&c).ok())
        } else {
            None
        }
    };

    let config = Config::build(Some(&working_dir), true)?;

    let vendor_dir = working_dir.join(&config.vendor_dir);
    let installed_repo = Arc::new(phpx_pm::repository::InstalledRepository::new(vendor_dir.clone()));
    installed_repo.load().await.ok();
    let installed_packages = installed_repo.get_packages().await;

    if args.self_package {
        if args.name_only {
            if let Some(name) = &composer_json.name {
                println!("{}", name);
            }
            return Ok(0);
        }

        if args.package.is_some() {
            eprintln!("Error: Cannot use --self together with a package name");
            return Ok(1);
        }

        print_root_package_info(&composer_json, &args.format)?;
        return Ok(0);
    }

    if args.locked {
        if lock.is_none() {
            eprintln!("Error: A valid composer.json and composer.lock is required for --locked");
            return Ok(1);
        }
    }

    if installed_packages.is_empty() && (!composer_json.require.is_empty() || !composer_json.require_dev.is_empty()) {
        eprintln!("Warning: No dependencies installed. Try running install or update.");
    }

    if let Some(package_name) = &args.package {
        if !package_name.contains('*') {
            show_single_package(
                &installed_packages,
                package_name,
                args.version.as_deref(),
                &args,
                &vendor_dir,
            )?;
        } else {
            list_packages_filtered(&installed_packages, Some(package_name), &composer_json, &args)?;
        }
    } else {
        if args.tree {
            show_tree_all(&installed_packages, &composer_json)?;
        } else {
            list_packages_filtered(&installed_packages, None, &composer_json, &args)?;
        }
    }

    Ok(0)
}

fn print_root_package_info(composer_json: &ComposerJson, format: &str) -> Result<()> {
    if format == "json" {
        let json = serde_json::json!({
            "name": composer_json.name,
            "version": composer_json.version,
            "description": composer_json.description,
            "type": composer_json.package_type,
            "license": composer_json.license,
            "require": composer_json.require,
            "require-dev": composer_json.require_dev,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        if let Some(name) = &composer_json.name {
            println!("name     : {}", name);
        }
        if let Some(desc) = &composer_json.description {
            println!("descrip. : {}", desc);
        }
        if let Some(version) = &composer_json.version {
            println!("version  : {}", version);
        }
        println!("type     : {}", &composer_json.package_type);

        if !composer_json.require.is_empty() {
            println!("\nrequires");
            for (name, constraint) in &composer_json.require {
                println!("{} {}", name, constraint);
            }
        }

        if !composer_json.require_dev.is_empty() {
            println!("\nrequires (dev)");
            for (name, constraint) in &composer_json.require_dev {
                println!("{} {}", name, constraint);
            }
        }
    }
    Ok(())
}

fn show_single_package(
    packages: &[Arc<phpx_pm::Package>],
    name: &str,
    _version: Option<&str>,
    args: &ShowArgs,
    vendor_dir: &PathBuf,
) -> Result<()> {
    let name_lower = name.to_lowercase();
    let package = packages
        .iter()
        .find(|p| p.name.to_lowercase() == name_lower);

    let package = match package {
        Some(p) => p,
        None => {
            eprintln!("Error: Package '{}' not found", name);
            return Ok(());
        }
    };

    if args.path {
        let install_path = vendor_dir.join(&package.name);
        if install_path.exists() {
            println!("{} {}", package.name, install_path.display());
        } else {
            println!("{} null", package.name);
        }
        return Ok(());
    }

    if args.tree {
        show_tree_single(package, packages)?;
        return Ok(());
    }

    if args.format == "json" {
        print_package_json(package)?;
    } else {
        print_package_info(package)?;
    }

    Ok(())
}

fn print_package_info(package: &phpx_pm::Package) -> Result<()> {
    println!("name     : {}", package.name);
    if let Some(desc) = &package.description {
        println!("descrip. : {}", desc);
    }
    println!("versions : {}", package.pretty_version.as_deref().unwrap_or(&package.version));
    println!("type     : {}", package.package_type);

    if !package.require.is_empty() {
        println!("\nrequires");
        for (name, constraint) in &package.require {
            println!("{} {}", name, constraint);
        }
    }

    if !package.require_dev.is_empty() {
        println!("\nrequires (dev)");
        for (name, constraint) in &package.require_dev {
            println!("{} {}", name, constraint);
        }
    }

    if !package.provide.is_empty() {
        println!("\nprovide");
        for (name, constraint) in &package.provide {
            println!("{} {}", name, constraint);
        }
    }

    if !package.conflict.is_empty() {
        println!("\nconflict");
        for (name, constraint) in &package.conflict {
            println!("{} {}", name, constraint);
        }
    }

    if !package.replace.is_empty() {
        println!("\nreplace");
        for (name, constraint) in &package.replace {
            println!("{} {}", name, constraint);
        }
    }

    Ok(())
}

fn print_package_json(package: &phpx_pm::Package) -> Result<()> {
    let json = serde_json::json!({
        "name": package.name,
        "version": package.pretty_version.as_deref().unwrap_or(&package.version),
        "description": package.description,
        "type": package.package_type,
        "require": package.require,
        "require-dev": package.require_dev,
        "provide": package.provide,
        "conflict": package.conflict,
        "replace": package.replace,
    });
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

fn list_packages_filtered(
    packages: &[Arc<phpx_pm::Package>],
    filter: Option<&str>,
    composer_json: &ComposerJson,
    args: &ShowArgs,
) -> Result<()> {
    let mut filtered: Vec<_> = packages
        .iter()
        .filter(|p| {
            if let Some(pattern) = filter {
                let regex_pattern = pattern.replace('*', ".*");
                let re = regex::Regex::new(&format!("^{}$", regex_pattern)).unwrap();
                re.is_match(&p.name.to_lowercase())
            } else {
                true
            }
        })
        .collect();

    if args.direct {
        let root_requires: Vec<String> = composer_json
            .require
            .keys()
            .chain(composer_json.require_dev.keys())
            .map(|s| s.to_lowercase())
            .collect();

        filtered.retain(|p| root_requires.contains(&p.name.to_lowercase()));
    }

    filtered.sort_by(|a, b| a.name.cmp(&b.name));

    if args.format == "json" {
        let json: Vec<_> = filtered
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "version": p.pretty_version.as_deref().unwrap_or(&p.version),
                    "description": p.description,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        for package in filtered {
            if args.name_only {
                println!("{}", package.name);
            } else {
                let version = package.pretty_version.as_deref().unwrap_or(&package.version);
                let desc = package
                    .description
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("");
                println!("{:<40} {:<15} {}", package.name, version, desc);
            }
        }
    }

    Ok(())
}

fn show_tree_single(package: &Arc<phpx_pm::Package>, all_packages: &[Arc<phpx_pm::Package>]) -> Result<()> {
    let version = package.pretty_version.as_deref().unwrap_or(&package.version);
    let desc = package.description.as_deref().unwrap_or("");
    println!("{} {} {}", package.name, version, desc);

    let mut visited = HashSet::new();
    visited.insert(package.name.to_lowercase());

    print_dependencies_tree(&package.require, all_packages, "", &mut visited);

    Ok(())
}

fn show_tree_all(packages: &[Arc<phpx_pm::Package>], composer_json: &ComposerJson) -> Result<()> {
    let root_requires: HashSet<String> = composer_json
        .require
        .keys()
        .chain(composer_json.require_dev.keys())
        .map(|s| s.to_lowercase())
        .collect();

    let mut root_packages: Vec<_> = packages
        .iter()
        .filter(|p| root_requires.contains(&p.name.to_lowercase()))
        .collect();

    root_packages.sort_by(|a, b| a.name.cmp(&b.name));

    for package in root_packages {
        let version = package.pretty_version.as_deref().unwrap_or(&package.version);
        println!("{} {}", package.name, version);

        let mut visited = HashSet::new();
        visited.insert(package.name.to_lowercase());

        print_dependencies_tree(&package.require, packages, "", &mut visited);
    }

    Ok(())
}

fn print_dependencies_tree(
    requires: &std::collections::HashMap<String, String>,
    all_packages: &[Arc<phpx_pm::Package>],
    prefix: &str,
    visited: &mut HashSet<String>,
) {
    let mut deps: Vec<_> = requires
        .iter()
        .filter(|(name, _)| !is_platform_package(name))
        .collect();
    deps.sort_by(|a, b| a.0.cmp(b.0));

    let count = deps.len();
    for (idx, (dep_name, constraint)) in deps.iter().enumerate() {
        let is_last = idx == count - 1;
        let branch = if is_last { "└──" } else { "├──" };

        let dep_lower = dep_name.to_lowercase();
        let package = all_packages.iter().find(|p| p.name.to_lowercase() == dep_lower);

        if let Some(pkg) = package {
            let version = pkg.pretty_version.as_deref().unwrap_or(&pkg.version);

            if visited.contains(&dep_lower) {
                println!("{}{} {} {} (circular dependency aborted here)", prefix, branch, dep_name, version);
            } else {
                println!("{}{} {} {} ({})", prefix, branch, dep_name, version, constraint);

                visited.insert(dep_lower.clone());

                let new_prefix = format!("{}{}   ", prefix, if is_last { " " } else { "│" });
                print_dependencies_tree(&pkg.require, all_packages, &new_prefix, visited);

                visited.remove(&dep_lower);
            }
        } else {
            println!("{}{} {} ({})", prefix, branch, dep_name, constraint);
        }
    }
}
