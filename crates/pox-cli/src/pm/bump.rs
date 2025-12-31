use anyhow::{Context, Result};
use clap::Args;
use indexmap::IndexMap;
use regex::Regex;
use std::path::PathBuf;

use pox_pm::json::{ComposerJson, ComposerLock};
use pox_pm::package::version_bumper::bump_requirement;
use pox_pm::{compute_content_hash, is_platform_package};

#[derive(Args, Debug)]
pub struct BumpArgs {
    /// Optional package name(s) to restrict which packages are bumped
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,

    /// Only bump requirements in "require-dev"
    #[arg(short = 'D', long)]
    pub dev_only: bool,

    /// Only bump requirements in "require"
    #[arg(short = 'R', long)]
    pub no_dev_only: bool,

    /// Outputs the packages to bump, but will not execute anything
    #[arg(long)]
    pub dry_run: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

pub struct BumpUpdates {
    pub require: IndexMap<String, String>,
    pub require_dev: IndexMap<String, String>,
}

pub fn calculate_updates(
    composer_json: &ComposerJson,
    lock: &ComposerLock,
    packages_filter: &[String],
    dev_only: bool,
    no_dev_only: bool,
) -> BumpUpdates {
    let mut updates = BumpUpdates {
        require: IndexMap::new(),
        require_dev: IndexMap::new(),
    };

    let filter_patterns: Vec<Regex> = packages_filter
        .iter()
        .filter_map(|p| {
            let name = p.split(':').next().unwrap_or(p);
            let pattern = name.replace('*', ".*").replace('?', ".");
            Regex::new(&format!("^{}$", pattern)).ok()
        })
        .collect();

    let matches_filter = |name: &str| -> bool {
        if filter_patterns.is_empty() {
            return true;
        }
        let name_lower = name.to_lowercase();
        filter_patterns.iter().any(|p| p.is_match(&name_lower))
    };

    if !dev_only {
        for (name, constraint) in &composer_json.require {
            if is_platform_package(name) {
                continue;
            }
            if !matches_filter(name) {
                continue;
            }

            if let Some(pkg) = lock.find_package(name) {
                let bumped = bump_requirement(constraint, &pkg.version);
                if bumped != *constraint {
                    updates.require.insert(name.clone(), bumped);
                }
            }
        }
    }

    if !no_dev_only {
        for (name, constraint) in &composer_json.require_dev {
            if is_platform_package(name) {
                continue;
            }
            if !matches_filter(name) {
                continue;
            }

            if let Some(pkg) = lock.find_package(name) {
                let bumped = bump_requirement(constraint, &pkg.version);
                if bumped != *constraint {
                    updates.require_dev.insert(name.clone(), bumped);
                }
            }
        }
    }

    updates
}

pub fn apply_updates_to_json(content: &str, updates: &BumpUpdates) -> Result<String> {
    let mut result = content.to_string();

    for (name, new_version) in &updates.require {
        result = update_dependency_in_json(&result, "require", name, new_version)?;
    }

    for (name, new_version) in &updates.require_dev {
        result = update_dependency_in_json(&result, "require-dev", name, new_version)?;
    }

    Ok(result)
}

fn update_dependency_in_json(
    content: &str,
    section: &str,
    name: &str,
    new_version: &str,
) -> Result<String> {
    let escaped_name = regex::escape(name);
    let pattern = format!(r#"("{}")\s*:\s*"([^"]*)""#, escaped_name);

    let re = Regex::new(&pattern).context("Failed to build regex pattern")?;

    let section_pattern = format!(r#""{}"\s*:\s*\{{"#, regex::escape(section));
    let section_re = Regex::new(&section_pattern)?;

    if let Some(section_match) = section_re.find(content) {
        let section_start = section_match.start();
        let remaining = &content[section_start..];
        let mut brace_count = 0;
        let mut section_end = remaining.len();

        for (i, ch) in remaining.chars().enumerate() {
            match ch {
                '{' => brace_count += 1,
                '}' => {
                    brace_count -= 1;
                    if brace_count == 0 {
                        section_end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }

        let section_content = &content[section_start..section_start + section_end];

        if let Some(caps) = re.captures(section_content) {
            let full_match = caps.get(0).unwrap();
            let replacement = format!(r#"{}": "{}""#, &caps[1], new_version);

            let new_section = format!(
                "{}{}{}",
                &section_content[..full_match.start()],
                replacement,
                &section_content[full_match.end()..]
            );

            return Ok(format!(
                "{}{}{}",
                &content[..section_start],
                new_section,
                &content[section_start + section_end..]
            ));
        }
    }

    if let Some(caps) = re.captures(content) {
        let full_match = caps.get(0).unwrap();
        let replacement = format!(r#"{}": "{}""#, &caps[1], new_version);

        return Ok(format!(
            "{}{}{}",
            &content[..full_match.start()],
            replacement,
            &content[full_match.end()..]
        ));
    }

    Ok(content.to_string())
}

pub async fn execute(args: BumpArgs) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

    let json_path = working_dir.join("composer.json");
    let lock_path = working_dir.join("composer.lock");

    if !json_path.exists() {
        eprintln!("./composer.json is not readable.");
        return Ok(1);
    }

    let json_content =
        std::fs::read_to_string(&json_path).context("Failed to read composer.json")?;

    let composer_json: ComposerJson =
        serde_json::from_str(&json_content).context("Failed to parse composer.json")?;

    if composer_json.package_type != "project" && !args.dev_only {
        eprintln!("Warning: Bumping dependency constraints is not recommended for libraries as it will narrow down your dependencies and may cause problems for your users.");
        if composer_json.package_type == "library" {
            eprintln!("If your package is not a library, you can explicitly specify the \"type\" by using \"composer config type project\".");
            eprintln!(
                "Alternatively you can use --dev-only to only bump dependencies within \"require-dev\"."
            );
        }
    }

    let lock: ComposerLock = if lock_path.exists() {
        let lock_content =
            std::fs::read_to_string(&lock_path).context("Failed to read composer.lock")?;
        serde_json::from_str(&lock_content).context("Failed to parse composer.lock")?
    } else {
        let installed_path = working_dir.join("vendor/composer/installed.json");
        if installed_path.exists() {
            let installed_content =
                std::fs::read_to_string(&installed_path).context("Failed to read installed.json")?;
            parse_installed_json(&installed_content)?
        } else {
            eprintln!("No composer.lock or vendor/composer/installed.json found.");
            eprintln!("Run 'pox install' first to create a lock file.");
            return Ok(1);
        }
    };

    let updates = calculate_updates(
        &composer_json,
        &lock,
        &args.packages,
        args.dev_only,
        args.no_dev_only,
    );

    let change_count = updates.require.len() + updates.require_dev.len();

    if change_count > 0 {
        if args.dry_run {
            println!("./composer.json would be updated with:");
            for (name, version) in &updates.require {
                println!("  - require.{}: {}", name, version);
            }
            for (name, version) in &updates.require_dev {
                println!("  - require-dev.{}: {}", name, version);
            }
            return Ok(1);
        }

        let new_content = apply_updates_to_json(&json_content, &updates)?;

        let metadata = std::fs::metadata(&json_path)?;
        if metadata.permissions().readonly() {
            eprintln!("./composer.json is not writable.");
            return Ok(1);
        }

        std::fs::write(&json_path, &new_content).context("Failed to write composer.json")?;

        println!(
            "./composer.json has been updated ({} changes).",
            change_count
        );

        if lock_path.exists() {
            update_lock_hash(&lock_path, &new_content)?;
        }
    } else {
        println!("No requirements to update in ./composer.json.");
    }

    Ok(0)
}

fn parse_installed_json(content: &str) -> Result<ComposerLock> {
    use pox_pm::json::LockedPackage;

    #[derive(serde::Deserialize)]
    struct InstalledJson {
        packages: Option<Vec<LockedPackage>>,
        #[serde(rename = "dev-package-names")]
        dev_package_names: Option<Vec<String>>,
    }

    if let Ok(installed) = serde_json::from_str::<InstalledJson>(content) {
        let all_packages = installed.packages.unwrap_or_default();
        let dev_names: std::collections::HashSet<String> = installed
            .dev_package_names
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.to_lowercase())
            .collect();

        let (dev_packages, packages): (Vec<_>, Vec<_>) = all_packages
            .into_iter()
            .partition(|p| dev_names.contains(&p.name.to_lowercase()));

        return Ok(ComposerLock {
            packages,
            packages_dev: dev_packages,
            ..Default::default()
        });
    }

    if let Ok(packages) = serde_json::from_str::<Vec<LockedPackage>>(content) {
        return Ok(ComposerLock {
            packages,
            ..Default::default()
        });
    }

    anyhow::bail!("Failed to parse installed.json")
}

fn update_lock_hash(lock_path: &std::path::Path, json_content: &str) -> Result<()> {
    let mut lock = ComposerLock::from_file(lock_path)
        .context("Failed to read composer.lock")?;

    lock.content_hash = compute_content_hash(json_content);

    let new_lock_content = lock.to_json()
        .context("Failed to serialize composer.lock")?;
    std::fs::write(lock_path, new_lock_content)?;

    Ok(())
}
