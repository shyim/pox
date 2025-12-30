//! Why command - show which packages depend on a given package.

use anyhow::{Context, Result};
use clap::Args;
use std::path::PathBuf;
use std::sync::Arc;

use pox_pm::{
    ComposerBuilder, DependencyResult, Repository,
    config::Config,
    find_packages_with_replacers_and_providers, get_dependents,
    is_platform_package,
    json::{ComposerJson, ComposerLock},
};

#[derive(Args, Debug)]
pub struct WhyArgs {
    /// Package name to analyze
    pub package: String,

    /// Version constraint (optional)
    pub constraint: Option<String>,

    /// Show the full dependency tree
    #[arg(short = 't', long)]
    pub tree: bool,

    /// Show recursive dependencies
    #[arg(short = 'r', long)]
    pub recursive: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

pub async fn execute(args: WhyArgs, inverted: bool) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

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
    let installed_repo = Arc::new(pox_pm::repository::InstalledRepository::new(vendor_dir));
    installed_repo.load().await.ok();
    let mut installed_packages = installed_repo.get_packages().await;

    let root_package = pox_pm::Package {
        name: composer_json.name.clone().unwrap_or_else(|| "__root__".to_string()),
        pretty_name: composer_json.name.clone(),
        version: composer_json.version.clone().unwrap_or_else(|| "dev-main".to_string()),
        pretty_version: composer_json.version.clone(),
        package_type: "root-package".to_string(),
        require: composer_json.require.clone(),
        require_dev: composer_json.require_dev.clone(),
        conflict: composer_json.conflict.clone(),
        replace: composer_json.replace.clone(),
        provide: composer_json.provide.clone(),
        ..Default::default()
    };
    installed_packages.push(Arc::new(root_package));

    let _composer = ComposerBuilder::new(working_dir.clone())
        .with_config(config)
        .with_composer_json(composer_json.clone())
        .with_composer_lock(lock.clone())
        .build()?;

    if installed_packages.is_empty() && (!composer_json.require.is_empty() || !composer_json.require_dev.is_empty()) {
        eprintln!("Warning: No dependencies installed. Try running install or update, or use --locked.");
        return Ok(1);
    }

    let needle = &args.package;
    let constraint_str = args.constraint.as_deref().unwrap_or("*");

    let constraint = if constraint_str != "*" {
        let parser = pox_semver::VersionParser;
        match parser.parse_constraints(constraint_str) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("Error: Invalid constraint '{}': {:?}", constraint_str, e);
                return Ok(1);
            }
        }
    } else {
        None
    };

    let matching_packages = find_packages_with_replacers_and_providers(
        &installed_packages,
        needle,
        constraint.as_ref().map(|v| &**v),
    );

    if matching_packages.is_empty() {
        eprintln!("Error: Could not find package \"{}\" in your project", needle);
        return Ok(1);
    }

    let matched_package = installed_packages.iter()
        .find(|p| p.name.to_lowercase() == needle.to_lowercase());

    if matched_package.is_some() && inverted {
        if let Some(pkg) = matched_package {
            println!(
                "Package \"{}\" {} is already installed! To find out why, run `phpx pm why {}`",
                needle,
                pkg.pretty_version.as_deref().unwrap_or(&pkg.version),
                needle
            );
            return Ok(0);
        }
    }

    let mut needles = vec![needle.to_string()];
    if inverted {
        for package in &matching_packages {
            for (target, _constraint) in &package.replace {
                needles.push(target.clone());
            }
        }
    }

    let recursive = args.tree || args.recursive;
    let results = get_dependents(
        &installed_packages,
        &needles,
        constraint.as_ref().map(|v| &**v),
        inverted,
        recursive,
        None,
    );

    if results.is_empty() {
        let extra = if constraint.is_some() {
            format!(
                " in versions {}matching {}",
                if inverted { "not " } else { "" },
                constraint_str
            )
        } else {
            String::new()
        };
        println!(
            "There is no installed package depending on \"{}\"{}",
            needle, extra
        );
        return Ok(if inverted { 0 } else { 1 });
    }

    if args.tree {
        print_tree(&results, &matching_packages[0]);
    } else {
        print_table(&results);
    }

    if inverted && args.constraint.is_some() && !is_platform_package(needle) {
        let mut command = "update";

        for req in &composer_json.require {
            if req.0.to_lowercase() == needle.to_lowercase() {
                command = "require";
                break;
            }
        }

        for req in &composer_json.require_dev {
            if req.0.to_lowercase() == needle.to_lowercase() {
                command = "require --dev";
                break;
            }
        }

        eprintln!(
            "\nNot finding what you were looking for? Try calling `phpx {} \"{}:{}\" --dry-run` to get another view on the problem.",
            command, needle, constraint_str
        );
    }

    Ok(if inverted { 1 } else { 0 })
}

fn print_table(results: &[DependencyResult]) {
    println!("{:<40} {:<15} {:<15} {}", "Package", "Version", "Dependency", "Constraint");
    println!("{}", "-".repeat(100));

    let mut seen = std::collections::HashSet::new();
    let mut all_results = Vec::new();
    let mut queue: Vec<&DependencyResult> = results.iter().collect();

    while !queue.is_empty() {
        let mut next_queue = Vec::new();

        for result in queue {
            let key = format!("{}:{}", result.package.name, result.link.target);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            all_results.push(result);

            if let Some(ref children) = result.children {
                next_queue.extend(children.iter());
            }
        }

        queue = next_queue;
    }

    all_results.sort_by(|a, b| {
        let a_is_root = a.package.package_type.as_str() == "root-package";
        let b_is_root = b.package.package_type.as_str() == "root-package";

        match (a_is_root, b_is_root) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.package.name.cmp(&b.package.name),
        }
    });

    for result in all_results {
        let version = result
            .package
            .pretty_version
            .as_deref()
            .unwrap_or(&result.package.version);

        let link_desc = result.link.link_type.description();

        println!(
            "{:<40} {:<15} {:<15} {}",
            result.package.name,
            version,
            link_desc,
            format!("{} ({})", result.link.target, result.link.constraint)
        );
    }
}

fn print_tree(results: &[DependencyResult], root: &Arc<pox_pm::Package>) {
    println!(
        "{} {}",
        root.name,
        root.pretty_version.as_deref().unwrap_or(&root.version)
    );
    print_tree_recursive(results, "", 0);
}

fn print_tree_recursive(results: &[DependencyResult], prefix: &str, _level: usize) {
    let count = results.len();

    for (idx, result) in results.iter().enumerate() {
        let is_last = idx == count - 1;
        let branch = if is_last { "└── " } else { "├── " };

        let version = result
            .package
            .pretty_version
            .as_deref()
            .unwrap_or(&result.package.version);

        let circular_warn = if result.children.is_none() {
            " (circular dependency aborted here)"
        } else {
            ""
        };

        let link_desc = result.link.link_type.description();

        println!(
            "{}{}{} {} ({} {} {}){}",
            prefix,
            branch,
            result.package.name,
            version,
            link_desc,
            result.link.target,
            result.link.constraint,
            circular_warn
        );

        if let Some(ref children) = result.children {
            let new_prefix = format!("{}{}   ", prefix, if is_last { " " } else { "│" });
            print_tree_recursive(children, &new_prefix, _level + 1);
        }
    }
}
