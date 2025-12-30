//! Show command - display package information.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};

use pox_pm::{
    Repository,
    config::Config,
    json::{ComposerJson, ComposerLock},
    is_platform_package,
    repository::ComposerRepository,
};
use pox_semver::VersionParser;

#[derive(Debug, Clone, Copy, PartialEq)]
enum UpdateType {
    UpToDate,
    Patch,
    Minor,
    Major,
}

fn determine_update_type(current: &str, latest: &str) -> UpdateType {
    let parser = VersionParser::new();
    let current_normalized = parser.normalize(current).unwrap_or_else(|_| current.to_string());
    let latest_normalized = parser.normalize(latest).unwrap_or_else(|_| latest.to_string());

    if current_normalized == latest_normalized {
        return UpdateType::UpToDate;
    }

    let current_parts: Vec<u64> = current_normalized
        .split('.')
        .filter_map(|s| s.split('-').next())
        .filter_map(|s| s.parse().ok())
        .collect();
    let latest_parts: Vec<u64> = latest_normalized
        .split('.')
        .filter_map(|s| s.split('-').next())
        .filter_map(|s| s.parse().ok())
        .collect();

    let current_major = current_parts.first().copied().unwrap_or(0);
    let current_minor = current_parts.get(1).copied().unwrap_or(0);
    let latest_major = latest_parts.first().copied().unwrap_or(0);
    let latest_minor = latest_parts.get(1).copied().unwrap_or(0);

    if latest_major > current_major {
        UpdateType::Major
    } else if latest_minor > current_minor {
        UpdateType::Minor
    } else {
        UpdateType::Patch
    }
}

struct PackageWithLatest {
    package: Arc<pox_pm::Package>,
    latest_version: Option<String>,
    update_type: UpdateType,
}

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
    let installed_repo = Arc::new(pox_pm::repository::InstalledRepository::new(vendor_dir.clone()));
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

    let show_latest = args.latest || args.outdated;

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
            list_packages_with_latest(&installed_packages, Some(package_name), &composer_json, &args, &config, show_latest).await?;
        }
    } else {
        if args.tree {
            show_tree_all(&installed_packages, &composer_json)?;
        } else {
            list_packages_with_latest(&installed_packages, None, &composer_json, &args, &config, show_latest).await?;
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
    packages: &[Arc<pox_pm::Package>],
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

fn print_package_info(package: &pox_pm::Package) -> Result<()> {
    println!("name     : {}", package.name);
    if let Some(desc) = &package.description {
        println!("descrip. : {}", desc);
    }
    println!("versions : {}", package.pretty_version.as_deref().unwrap_or(&package.version));
    println!("type     : {}", package.package_type);

    if let Some(abandoned) = &package.abandoned {
        let replacement = match abandoned.replacement() {
            Some(pkg) => format!("Use {} instead", pkg),
            None => "No replacement was suggested".to_string(),
        };
        eprintln!("\nPackage {} is abandoned, you should avoid using it. {}.", package.name, replacement);
    }

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

fn print_package_json(package: &pox_pm::Package) -> Result<()> {
    let abandoned_value = package.abandoned.as_ref().map(|a| {
        match a.replacement() {
            Some(pkg) => serde_json::json!(pkg),
            None => serde_json::json!(true),
        }
    });

    let json = serde_json::json!({
        "name": package.name,
        "version": package.pretty_version.as_deref().unwrap_or(&package.version),
        "description": package.description,
        "type": package.package_type,
        "abandoned": abandoned_value,
        "require": package.require,
        "require-dev": package.require_dev,
        "provide": package.provide,
        "conflict": package.conflict,
        "replace": package.replace,
    });
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

async fn fetch_latest_versions(
    packages: &[Arc<pox_pm::Package>],
    config: &Config,
) -> HashMap<String, String> {
    let mut latest_versions = HashMap::new();

    let packagist = if let Some(cache_dir) = &config.cache_dir {
        ComposerRepository::packagist_with_cache(cache_dir.join("repo"))
    } else {
        ComposerRepository::packagist()
    };

    for pkg in packages {
        if is_platform_package(&pkg.name) {
            continue;
        }

        let versions = packagist.find_packages(&pkg.name).await;
        if let Some(latest) = find_latest_stable_version(&versions) {
            latest_versions.insert(pkg.name.to_lowercase(), latest);
        }
    }

    latest_versions
}

fn find_latest_stable_version(packages: &[Arc<pox_pm::Package>]) -> Option<String> {
    let parser = VersionParser::new();

    let mut stable_versions: Vec<_> = packages
        .iter()
        .filter(|p| {
            let version = p.pretty_version.as_deref().unwrap_or(&p.version);
            !version.contains("dev")
                && !version.contains("alpha")
                && !version.contains("beta")
                && !version.contains("RC")
        })
        .collect();

    stable_versions.sort_by(|a, b| {
        let v_a = a.pretty_version.as_deref().unwrap_or(&a.version);
        let v_b = b.pretty_version.as_deref().unwrap_or(&b.version);

        let norm_a = parser.normalize(v_a).unwrap_or_else(|_| v_a.to_string());
        let norm_b = parser.normalize(v_b).unwrap_or_else(|_| v_b.to_string());

        compare_versions(&norm_b, &norm_a)
    });

    stable_versions.first().map(|p| {
        p.pretty_version.as_deref().unwrap_or(&p.version).to_string()
    })
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<u64> = a.split('.').filter_map(|s| s.split('-').next()).filter_map(|s| s.parse().ok()).collect();
    let b_parts: Vec<u64> = b.split('.').filter_map(|s| s.split('-').next()).filter_map(|s| s.parse().ok()).collect();

    for i in 0..std::cmp::max(a_parts.len(), b_parts.len()) {
        let a_part = a_parts.get(i).copied().unwrap_or(0);
        let b_part = b_parts.get(i).copied().unwrap_or(0);
        match a_part.cmp(&b_part) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

fn strip_version_prefix(version: &str) -> &str {
    version.strip_prefix('v').or_else(|| version.strip_prefix('V')).unwrap_or(version)
}

async fn list_packages_with_latest(
    packages: &[Arc<pox_pm::Package>],
    filter: Option<&str>,
    composer_json: &ComposerJson,
    args: &ShowArgs,
    config: &Config,
    show_latest: bool,
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
        .cloned()
        .collect();

    let root_requires: HashSet<String> = composer_json
        .require
        .keys()
        .map(|s| s.to_lowercase())
        .collect();

    let root_requires_dev: HashSet<String> = composer_json
        .require_dev
        .keys()
        .map(|s| s.to_lowercase())
        .collect();

    if args.direct {
        filtered.retain(|p| {
            let name = p.name.to_lowercase();
            root_requires.contains(&name) || root_requires_dev.contains(&name)
        });
    }

    filtered.sort_by(|a, b| a.name.cmp(&b.name));

    let latest_versions = if show_latest {
        fetch_latest_versions(&filtered, config).await
    } else {
        HashMap::new()
    };

    let mut packages_with_latest: Vec<PackageWithLatest> = filtered
        .into_iter()
        .map(|p| {
            let current = p.pretty_version.as_deref().unwrap_or(&p.version);
            let latest = latest_versions.get(&p.name.to_lowercase()).cloned();
            let update_type = if let Some(ref lat) = latest {
                determine_update_type(current, lat)
            } else {
                UpdateType::UpToDate
            };
            PackageWithLatest {
                package: p,
                latest_version: latest,
                update_type,
            }
        })
        .collect();

    if args.outdated {
        packages_with_latest.retain(|p| p.update_type != UpdateType::UpToDate);
    }

    if packages_with_latest.is_empty() {
        return Ok(());
    }

    if args.format == "json" {
        let json: Vec<_> = packages_with_latest
            .iter()
            .map(|p| {
                let abandoned_value = p.package.abandoned.as_ref().map(|a| {
                    match a.replacement() {
                        Some(pkg) => serde_json::json!(pkg),
                        None => serde_json::json!(true),
                    }
                });

                let mut obj = serde_json::json!({
                    "name": p.package.name,
                    "version": p.package.pretty_version.as_deref().unwrap_or(&p.package.version),
                    "description": p.package.description,
                    "abandoned": abandoned_value,
                });

                if let Some(ref latest) = p.latest_version {
                    obj["latest"] = serde_json::json!(latest);
                    obj["latest-status"] = serde_json::json!(match p.update_type {
                        UpdateType::UpToDate => "up-to-date",
                        UpdateType::Patch | UpdateType::Minor => "semver-safe-update",
                        UpdateType::Major => "update-possible",
                    });
                }

                obj
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        if show_latest && !args.name_only {
            eprintln!("{}", style("Color legend:").green());
            eprintln!("- {} release available - update recommended", style("patch or minor").red());
            eprintln!("- {} release available - update possible", style("major").yellow());
            eprintln!();

            let direct: Vec<_> = packages_with_latest
                .iter()
                .filter(|p| root_requires.contains(&p.package.name.to_lowercase()) || root_requires_dev.contains(&p.package.name.to_lowercase()))
                .collect();

            let transitive: Vec<_> = packages_with_latest
                .iter()
                .filter(|p| !root_requires.contains(&p.package.name.to_lowercase()) && !root_requires_dev.contains(&p.package.name.to_lowercase()))
                .collect();

            if !direct.is_empty() {
                eprintln!("{}", style("Direct dependencies required in composer.json:").green());
                print_packages_list(&direct, args);
            }

            if !transitive.is_empty() && !args.direct {
                if !direct.is_empty() {
                    println!();
                }
                eprintln!("{}", style("Transitive dependencies not required in composer.json:").green());
                print_packages_list(&transitive, args);
            }
        } else {
            print_packages_list(&packages_with_latest.iter().collect::<Vec<_>>(), args);
        }
    }

    Ok(())
}

fn make_packagist_link(name: &str) -> String {
    format!("https://packagist.org/packages/{}", name)
}

fn terminal_link(text: &str, url: &str) -> String {
    use console::Term;
    let term = Term::stdout();
    if term.is_term() {
        format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", url, text)
    } else {
        text.to_string()
    }
}

fn print_packages_list(packages: &[&PackageWithLatest], args: &ShowArgs) {
    let name_width = packages
        .iter()
        .map(|p| p.package.name.len())
        .max()
        .unwrap_or(30)
        .max(30);

    for pwl in packages {
        let package = &pwl.package;
        if args.name_only {
            println!("{}", package.name);
        } else {
            let raw_version = package.pretty_version.as_deref().unwrap_or(&package.version);
            let version = strip_version_prefix(raw_version);
            let desc = package
                .description
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");

            let link_url = make_packagist_link(&package.name);
            let linked_name = terminal_link(&package.name, &link_url);
            let padding = " ".repeat(name_width.saturating_sub(package.name.len()));

            if let Some(ref latest) = pwl.latest_version {
                let latest_display = strip_version_prefix(latest);
                let truncated_desc = if desc.len() > 30 {
                    format!("{}...", &desc[..27])
                } else {
                    desc.to_string()
                };

                let (colored_version, indicator, colored_latest) = match pwl.update_type {
                    UpdateType::UpToDate => (
                        style(version).green().to_string(),
                        style("=").green().to_string(),
                        style(latest_display).green().to_string(),
                    ),
                    UpdateType::Patch | UpdateType::Minor => (
                        style(version).red().to_string(),
                        style("!").red().to_string(),
                        style(latest_display).red().to_string(),
                    ),
                    UpdateType::Major => (
                        style(version).yellow().to_string(),
                        style("~").yellow().to_string(),
                        style(latest_display).yellow().to_string(),
                    ),
                };

                println!(
                    "{}{} {:<7} {} {:<7} {}",
                    linked_name, padding, colored_version, indicator, colored_latest, truncated_desc
                );
            } else {
                let abandoned_marker = if package.abandoned.is_some() {
                    format!(" {}", style("[abandoned]").red())
                } else {
                    String::new()
                };
                println!("{}{} {:<15} {}{}", linked_name, padding, version, desc, abandoned_marker);
            }
        }
    }
}

fn show_tree_single(package: &Arc<pox_pm::Package>, all_packages: &[Arc<pox_pm::Package>]) -> Result<()> {
    let version = package.pretty_version.as_deref().unwrap_or(&package.version);
    let desc = package.description.as_deref().unwrap_or("");
    println!("{} {} {}", package.name, version, desc);

    let mut visited = HashSet::new();
    visited.insert(package.name.to_lowercase());

    print_dependencies_tree(&package.require, all_packages, "", &mut visited);

    Ok(())
}

fn show_tree_all(packages: &[Arc<pox_pm::Package>], composer_json: &ComposerJson) -> Result<()> {
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
    requires: &indexmap::IndexMap<String, String>,
    all_packages: &[Arc<pox_pm::Package>],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_update_type_up_to_date() {
        assert_eq!(determine_update_type("1.0.0", "1.0.0"), UpdateType::UpToDate);
        assert_eq!(determine_update_type("v2.3.4", "2.3.4"), UpdateType::UpToDate);
    }

    #[test]
    fn test_determine_update_type_patch() {
        assert_eq!(determine_update_type("1.0.0", "1.0.1"), UpdateType::Patch);
        assert_eq!(determine_update_type("1.0.0", "1.0.5"), UpdateType::Patch);
    }

    #[test]
    fn test_determine_update_type_minor() {
        assert_eq!(determine_update_type("1.0.0", "1.1.0"), UpdateType::Minor);
        assert_eq!(determine_update_type("1.0.0", "1.5.3"), UpdateType::Minor);
    }

    #[test]
    fn test_determine_update_type_major() {
        assert_eq!(determine_update_type("1.0.0", "2.0.0"), UpdateType::Major);
        assert_eq!(determine_update_type("1.5.3", "3.0.0"), UpdateType::Major);
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("1.0.0", "1.0.0"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("1.0.1", "1.0.0"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.1"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("2.0.0", "1.9.9"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("1.10.0", "1.9.0"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_compare_versions_with_prefix() {
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_strip_version_prefix() {
        assert_eq!(strip_version_prefix("v1.0.0"), "1.0.0");
        assert_eq!(strip_version_prefix("V2.3.4"), "2.3.4");
        assert_eq!(strip_version_prefix("1.0.0"), "1.0.0");
        assert_eq!(strip_version_prefix("v7.3.8"), "7.3.8");
    }
}
