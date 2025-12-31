//! Reinstall command - uninstall and reinstall packages.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use regex::Regex;
use std::path::PathBuf;

use pox_pm::{
    ComposerBuilder,
    config::Config,
    json::{ComposerJson, ComposerLock},
    package::Package,
};

use crate::pm::platform::PlatformInfo;

#[derive(Args, Debug)]
pub struct ReinstallArgs {
    /// Package names to reinstall (supports wildcards like "acme/*")
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,

    /// Reinstall packages by type (e.g., "library", "symfony-bundle")
    #[arg(long = "type", value_name = "TYPE")]
    pub package_types: Vec<String>,

    /// Prefer source installation (git clone)
    #[arg(long)]
    pub prefer_source: bool,

    /// Prefer dist installation (zip download)
    #[arg(long)]
    pub prefer_dist: bool,

    /// Skip autoloader generation
    #[arg(long)]
    pub no_autoloader: bool,

    /// Disable progress output
    #[arg(long)]
    pub no_progress: bool,

    /// Optimize autoloader (convert PSR-4/PSR-0 to classmap)
    #[arg(short = 'o', long)]
    pub optimize_autoloader: bool,

    /// Use authoritative classmap (only load from classmap)
    #[arg(short = 'a', long)]
    pub classmap_authoritative: bool,

    /// Use APCu to cache found/not-found classes
    #[arg(long)]
    pub apcu_autoloader: bool,

    /// Use a custom prefix for the APCu autoloader cache
    #[arg(long)]
    pub apcu_autoloader_prefix: Option<String>,

    /// Ignore platform requirements
    #[arg(long)]
    pub ignore_platform_reqs: bool,

    /// Ignore specific platform requirements
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    pub ignore_platform_req: Vec<String>,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,

    /// Do not ask any interactive question
    #[arg(short = 'n', long)]
    pub no_interaction: bool,

    /// Do not output any message
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Disable plugins
    #[arg(long)]
    pub no_plugins: bool,
}

fn package_name_to_regexp(pattern: &str) -> Result<Regex> {
    let escaped = regex::escape(pattern);
    let regex_pattern = escaped.replace(r"\*", ".*");
    let full_pattern = format!("(?i)^{}$", regex_pattern);
    Regex::new(&full_pattern).context("Invalid package pattern")
}

pub async fn execute(args: ReinstallArgs) -> Result<i32> {
    let working_dir = args.working_dir.canonicalize()
        .context("Failed to resolve working directory")?;

    let json_path = working_dir.join("composer.json");
    let composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
        anyhow::bail!("No composer.json found in the current directory");
    };

    let lock_path = working_dir.join("composer.lock");
    let lock: ComposerLock = if lock_path.exists() {
        let lock_content = std::fs::read_to_string(&lock_path)
            .context("Failed to read composer.lock")?;
        serde_json::from_str(&lock_content)
            .context("Failed to parse composer.lock")?
    } else {
        anyhow::bail!("No composer.lock found. Run 'install' or 'update' first.");
    };

    if !args.package_types.is_empty() && !args.packages.is_empty() {
        anyhow::bail!("You cannot specify package names and filter by type at the same time.");
    }

    if args.package_types.is_empty() && args.packages.is_empty() {
        anyhow::bail!("You must pass one or more package names to be reinstalled, or use --type to reinstall by package type.");
    }

    let mut packages_to_reinstall: Vec<Package> = Vec::new();
    let mut package_names_to_reinstall: Vec<String> = Vec::new();

    if !args.package_types.is_empty() {
        for locked_pkg in lock.packages.iter().chain(lock.packages_dev.iter()) {
            if args.package_types.contains(&locked_pkg.package_type) {
                packages_to_reinstall.push(Package::from(locked_pkg));
                package_names_to_reinstall.push(locked_pkg.name.clone());
            }
        }
    } else {
        for pattern in &args.packages {
            let pattern_regex = package_name_to_regexp(pattern)?;
            let mut matched = false;

            for locked_pkg in lock.packages.iter().chain(lock.packages_dev.iter()) {
                if pattern_regex.is_match(&locked_pkg.name) {
                    matched = true;
                    if !package_names_to_reinstall.iter().any(|n| n.eq_ignore_ascii_case(&locked_pkg.name)) {
                        packages_to_reinstall.push(Package::from(locked_pkg));
                        package_names_to_reinstall.push(locked_pkg.name.clone());
                    }
                }
            }

            if !matched {
                eprintln!(
                    "{} Pattern \"{}\" does not match any currently installed packages.",
                    style("Warning:").yellow(),
                    pattern
                );
            }
        }
    }

    if packages_to_reinstall.is_empty() {
        eprintln!("{} Found no packages to reinstall, aborting.", style("Warning:").yellow());
        return Ok(1);
    }

    println!(
        "{} Reinstalling {} package(s)",
        style("Composer").green().bold(),
        packages_to_reinstall.len()
    );

    let config = Config::build(Some(&working_dir), true)?;
    let platform = PlatformInfo::detect();

    let mut builder = ComposerBuilder::new(working_dir.clone())
        .with_config(config)
        .with_composer_json(composer_json)
        .with_composer_lock(Some(lock.clone()))
        .with_platform_packages(platform.to_packages());

    if args.prefer_source {
        builder = builder.prefer_source(true);
    } else if args.prefer_dist {
        builder = builder.prefer_dist(true);
    }

    let composer = builder.build()?;
    let manager = &composer.installation_manager;
    let vendor_dir = manager.config().vendor_dir.clone();

    println!("{} Removing packages...", style("Info:").cyan());
    for pkg in &packages_to_reinstall {
        let install_path = vendor_dir.join(&pkg.name);
        if install_path.exists() {
            tokio::fs::remove_dir_all(&install_path).await
                .with_context(|| format!("Failed to remove {}", pkg.name))?;
            println!(
                "  {} {} ({})",
                style("-").red(),
                style(&pkg.name).white().bold(),
                style(&pkg.version).yellow()
            );
        }
    }

    println!("{} Installing packages...", style("Info:").cyan());
    let result = manager.install_packages(&packages_to_reinstall).await
        .context("Failed to reinstall packages")?;

    for pkg in &result.installed {
        println!(
            "  {} {} ({})",
            style("+").green(),
            style(&pkg.name).white().bold(),
            style(&pkg.version).yellow()
        );
    }

    if !args.no_autoloader {
        println!("{} Generating autoload files", style("Info:").cyan());

        let installer = pox_pm::installer::Installer::new(composer);
        installer.dump_autoload(
            args.optimize_autoloader || args.classmap_authoritative,
            args.classmap_authoritative,
            args.apcu_autoloader || args.apcu_autoloader_prefix.is_some(),
            false,
        )?;
    }

    println!(
        "{} {} package(s) reinstalled",
        style("Success:").green().bold(),
        packages_to_reinstall.len()
    );

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_name_to_regexp_exact_match() {
        let re = package_name_to_regexp("root/req").unwrap();
        assert!(re.is_match("root/req"));
        assert!(re.is_match("ROOT/REQ")); // case insensitive
        assert!(!re.is_match("root/req2"));
        assert!(!re.is_match("other/req"));
    }

    #[test]
    fn test_package_name_to_regexp_wildcard_suffix() {
        let re = package_name_to_regexp("root/anotherreq*").unwrap();
        assert!(re.is_match("root/anotherreq"));
        assert!(re.is_match("root/anotherreq2"));
        assert!(re.is_match("root/anotherreq-foo"));
        assert!(!re.is_match("root/other"));
        assert!(!re.is_match("other/anotherreq"));
    }

    #[test]
    fn test_package_name_to_regexp_wildcard_vendor() {
        let re = package_name_to_regexp("acme/*").unwrap();
        assert!(re.is_match("acme/foo"));
        assert!(re.is_match("acme/bar"));
        assert!(re.is_match("ACME/FOO")); // case insensitive
        assert!(!re.is_match("other/foo"));
    }

    #[test]
    fn test_package_name_to_regexp_wildcard_middle() {
        let re = package_name_to_regexp("symfony/*-bundle").unwrap();
        assert!(re.is_match("symfony/framework-bundle"));
        assert!(re.is_match("symfony/twig-bundle"));
        assert!(!re.is_match("symfony/console"));
    }

    #[test]
    fn test_package_name_to_regexp_escapes_special_chars() {
        let re = package_name_to_regexp("vendor/pkg.name").unwrap();
        assert!(re.is_match("vendor/pkg.name"));
        assert!(!re.is_match("vendor/pkgXname")); // . should be literal, not any char
    }
}
