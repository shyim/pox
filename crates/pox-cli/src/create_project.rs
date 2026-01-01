//! Create-project command - create a new project from a package.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;
use std::sync::Arc;

use pox_pm::{
    ComposerBuilder,
    Repository,
    config::Config,
    downloader::{DownloadConfig, DownloadManager},
    http::HttpClient,
    installer::Installer,
    json::ComposerJson,
    repository::ComposerRepository,
    Package,
};
use pox_semver::VersionParser;

use crate::pm::platform::PlatformInfo;

#[derive(Args, Debug)]
pub struct CreateProjectArgs {
    /// Package name to be installed
    #[arg(value_name = "PACKAGE")]
    pub package: String,

    /// Directory where the files should be created
    #[arg(value_name = "DIRECTORY")]
    pub directory: Option<String>,

    /// Version, will default to latest
    #[arg(value_name = "VERSION")]
    pub version: Option<String>,

    /// Minimum-stability allowed (unless a version is specified)
    #[arg(short = 's', long)]
    pub stability: Option<String>,

    /// Prefer source installation (git clone)
    #[arg(long)]
    pub prefer_source: bool,

    /// Prefer dist installation (zip download)
    #[arg(long)]
    pub prefer_dist: bool,

    /// Add custom repositories
    #[arg(long, action = clap::ArgAction::Append)]
    pub repository: Vec<String>,

    /// Disables installation of require-dev packages
    #[arg(long)]
    pub no_dev: bool,

    /// Skip script execution
    #[arg(long)]
    pub no_scripts: bool,

    /// Disable progress output
    #[arg(long)]
    pub no_progress: bool,

    /// Skip installation of the package dependencies
    #[arg(long)]
    pub no_install: bool,

    /// Keep VCS metadata (.git, .svn, etc.)
    #[arg(long)]
    pub keep_vcs: bool,

    /// Force deletion of VCS metadata without prompting
    #[arg(long)]
    pub remove_vcs: bool,

    /// Ignore platform requirements
    #[arg(long)]
    pub ignore_platform_reqs: bool,

    /// Ignore specific platform requirements
    #[arg(long = "ignore-platform-req", value_name = "REQ")]
    pub ignore_platform_req: Vec<String>,

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

    /// Skip auditing of the installed package dependencies
    #[arg(long)]
    pub no_audit: bool,

    /// Audit output format (table, plain, json, or summary)
    #[arg(long, default_value = "summary")]
    pub audit_format: String,
}

fn parse_package_spec(package: &str) -> (String, Option<String>) {
    if let Some(pos) = package.find(':') {
        let name = package[..pos].to_string();
        let version = package[pos + 1..].to_string();
        (name, Some(version))
    } else if let Some(pos) = package.find('=') {
        let name = package[..pos].to_string();
        let version = package[pos + 1..].to_string();
        (name, Some(version))
    } else {
        (package.to_string(), None)
    }
}

fn find_best_version(
    packages: &[Arc<Package>],
    version_constraint: Option<&str>,
    stability: &str,
) -> Option<Arc<Package>> {
    let parser = VersionParser::new();

    let stability_priority = |s: &str| -> i32 {
        match s.to_lowercase().as_str() {
            "stable" => 0,
            "rc" => 1,
            "beta" => 2,
            "alpha" => 3,
            "dev" => 4,
            _ => 5,
        }
    };

    let min_stability = stability_priority(stability);

    let mut candidates: Vec<_> = packages
        .iter()
        .filter(|p| {
            let version = p.pretty_version.as_deref().unwrap_or(&p.version);
            let pkg_stability = get_version_stability(version);
            stability_priority(&pkg_stability) <= min_stability
        })
        .filter(|p| {
            if let Some(constraint) = version_constraint {
                if let Ok(parsed) = parser.parse_constraints_cached(constraint) {
                    return parsed.matches_normalized(&p.version);
                }
            }
            true
        })
        .cloned()
        .collect();

    candidates.sort_by(|a, b| {
        compare_versions(&b.version, &a.version)
    });

    candidates.into_iter().next()
}

fn get_version_stability(version: &str) -> String {
    let lower = version.to_lowercase();
    if lower.contains("dev") {
        "dev".to_string()
    } else if lower.contains("alpha") {
        "alpha".to_string()
    } else if lower.contains("beta") {
        "beta".to_string()
    } else if lower.contains("-rc") || lower.contains("rc") {
        "rc".to_string()
    } else {
        "stable".to_string()
    }
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<u64> = a
        .split('.')
        .filter_map(|s| s.split('-').next())
        .filter_map(|s| s.parse().ok())
        .collect();
    let b_parts: Vec<u64> = b
        .split('.')
        .filter_map(|s| s.split('-').next())
        .filter_map(|s| s.parse().ok())
        .collect();

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

pub async fn execute(args: CreateProjectArgs) -> Result<i32> {
    let (package_name, version_from_spec) = parse_package_spec(&args.package);
    let version_constraint = args.version.as_deref().or(version_from_spec.as_deref());

    let stability = args.stability.as_deref().unwrap_or("stable");

    let directory = args.directory.clone().unwrap_or_else(|| {
        package_name
            .split('/')
            .last()
            .unwrap_or(&package_name)
            .to_string()
    });

    let target_dir = std::env::current_dir()?.join(&directory);

    if target_dir.exists() {
        if !target_dir.is_dir() {
            anyhow::bail!(
                "Cannot create project directory at \"{}\", it exists as a file.",
                target_dir.display()
            );
        }
        if target_dir.read_dir()?.next().is_some() {
            anyhow::bail!(
                "Project directory \"{}\" is not empty.",
                target_dir.display()
            );
        }
    }

    println!(
        "{} Creating a \"{}\" project at \"{}\"",
        style("Info:").cyan(),
        package_name,
        directory
    );

    let config = Config::build(None::<&std::path::Path>, true)?;

    let repo = if let Some(cache_dir) = &config.cache_dir {
        ComposerRepository::packagist_with_cache(cache_dir.join("repo"))
    } else {
        ComposerRepository::packagist()
    };

    let packages = repo.find_packages(&package_name).await;

    if packages.is_empty() {
        anyhow::bail!("Could not find package {} with stability {}", package_name, stability);
    }

    let best_package = find_best_version(&packages, version_constraint, stability)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Could not find package {} with {} in a version matching {}",
                package_name,
                if let Some(v) = version_constraint {
                    format!("version {}", v)
                } else {
                    format!("stability {}", stability)
                },
                "your requirements"
            )
        })?;

    let version_display = best_package
        .pretty_version
        .as_deref()
        .unwrap_or(&best_package.version);

    println!(
        "{} Installing {} ({})",
        style("Info:").cyan(),
        style(&package_name).white().bold(),
        style(version_display).yellow()
    );

    std::fs::create_dir_all(&target_dir)?;

    let http_client = Arc::new(HttpClient::new().context("Failed to create HTTP client")?);
    let download_config = DownloadConfig {
        prefer_source: args.prefer_source,
        prefer_dist: args.prefer_dist || !args.prefer_source,
        cache_dir: config.cache_dir.clone().unwrap_or_else(|| PathBuf::from(".composer/cache")),
        vendor_dir: target_dir.clone(),
    };
    let download_manager = DownloadManager::new(http_client, download_config);

    let mut pkg_to_download = Package::new(&best_package.name, &best_package.version);
    pkg_to_download.dist = best_package.dist.clone();
    pkg_to_download.source = best_package.source.clone();

    download_manager
        .download(&pkg_to_download)
        .await
        .context("Failed to download package")?;

    let extracted_dir = target_dir.join(&best_package.name);
    if extracted_dir.exists() && extracted_dir != target_dir {
        for entry in std::fs::read_dir(&extracted_dir)? {
            let entry = entry?;
            let dest = target_dir.join(entry.file_name());
            std::fs::rename(entry.path(), dest)?;
        }
        std::fs::remove_dir_all(&extracted_dir)?;
    }

    println!("{} Created project in {}", style("Info:").cyan(), target_dir.display());

    if !args.keep_vcs && !args.prefer_source {
        let vcs_dirs = [".git", ".svn", ".hg", ".bzr", "_darcs", "CVS"];
        for vcs_dir in &vcs_dirs {
            let vcs_path = target_dir.join(vcs_dir);
            if vcs_path.exists() {
                let should_remove = args.remove_vcs
                    || args.no_interaction
                    || {
                        use dialoguer::Confirm;
                        Confirm::new()
                            .with_prompt("Do you want to remove the existing VCS (.git, .svn..) history?")
                            .default(true)
                            .interact()
                            .unwrap_or(true)
                    };

                if should_remove {
                    std::fs::remove_dir_all(&vcs_path).ok();
                }
            }
        }
    }

    if args.no_install {
        println!(
            "{} Skipping installation. Run 'pox install' in {} to install dependencies.",
            style("Info:").cyan(),
            directory
        );
        return Ok(0);
    }

    let composer_json_path = target_dir.join("composer.json");
    if !composer_json_path.exists() {
        println!(
            "{} No composer.json found in the package, skipping dependency installation.",
            style("Warning:").yellow()
        );
        return Ok(0);
    }

    println!("{} Installing dependencies...", style("Info:").cyan());

    let json_content = std::fs::read_to_string(&composer_json_path)?;
    let composer_json: ComposerJson = serde_json::from_str(&json_content)?;

    let project_config = Config::build(Some(&target_dir), true)?;

    let lock_path = target_dir.join("composer.lock");
    let has_lock = lock_path.exists();

    let platform = PlatformInfo::detect();

    let mut builder = ComposerBuilder::new(target_dir.clone())
        .with_config(project_config)
        .with_composer_json(composer_json)
        .with_platform_packages(platform.to_packages())
        .no_dev(args.no_dev);

    if args.prefer_source {
        builder = builder.prefer_source(true);
    } else if args.prefer_dist {
        builder = builder.prefer_dist(true);
    }

    let composer = builder.build()?;
    let installer = Installer::new(composer);

    let result = if has_lock {
        installer
            .install(args.no_scripts, false, false, false, args.ignore_platform_reqs)
            .await
    } else {
        installer.update(false, false, None).await
    };

    if result.is_ok() && !args.no_audit {
        let audit_args = crate::pm::audit::AuditArgs {
            no_dev: args.no_dev,
            format: args.audit_format.clone(),
            locked: false,
            abandoned: Some("report".to_string()),
            working_dir: target_dir.clone(),
        };

        if let Err(e) = crate::pm::audit::execute(audit_args).await {
            eprintln!("Warning: Audit failed: {}", e);
        }
    }

    println!(
        "\n{} Project {} successfully created in {}",
        style("Success:").green().bold(),
        style(&package_name).white().bold(),
        style(&directory).cyan()
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package_spec_with_colon() {
        let (name, version) = parse_package_spec("vendor/package:^1.0");
        assert_eq!(name, "vendor/package");
        assert_eq!(version, Some("^1.0".to_string()));
    }

    #[test]
    fn test_parse_package_spec_with_equals() {
        let (name, version) = parse_package_spec("vendor/package=1.0.0");
        assert_eq!(name, "vendor/package");
        assert_eq!(version, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_parse_package_spec_without_version() {
        let (name, version) = parse_package_spec("vendor/package");
        assert_eq!(name, "vendor/package");
        assert_eq!(version, None);
    }

    #[test]
    fn test_get_version_stability_stable() {
        assert_eq!(get_version_stability("1.0.0"), "stable");
        assert_eq!(get_version_stability("2.3.4"), "stable");
    }

    #[test]
    fn test_get_version_stability_dev() {
        assert_eq!(get_version_stability("dev-main"), "dev");
        assert_eq!(get_version_stability("1.0.0-dev"), "dev");
    }

    #[test]
    fn test_get_version_stability_alpha() {
        assert_eq!(get_version_stability("1.0.0-alpha1"), "alpha");
        assert_eq!(get_version_stability("1.0.0-alpha"), "alpha");
    }

    #[test]
    fn test_get_version_stability_beta() {
        assert_eq!(get_version_stability("1.0.0-beta1"), "beta");
        assert_eq!(get_version_stability("1.0.0-beta"), "beta");
    }

    #[test]
    fn test_get_version_stability_rc() {
        assert_eq!(get_version_stability("1.0.0-RC1"), "rc");
        assert_eq!(get_version_stability("1.0.0-rc1"), "rc");
    }

    #[test]
    fn test_compare_versions() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("2.0.0", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "2.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.1.0", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.1", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn test_find_best_version_selects_highest_stable() {
        let packages = vec![
            Arc::new(Package::new("vendor/pkg", "1.0.0.0")),
            Arc::new(Package::new("vendor/pkg", "2.0.0.0")),
            Arc::new(Package::new("vendor/pkg", "1.5.0.0")),
        ];

        let best = find_best_version(&packages, None, "stable");
        assert!(best.is_some());
        assert_eq!(best.unwrap().version, "2.0.0.0");
    }

    #[test]
    fn test_find_best_version_filters_by_constraint() {
        let packages = vec![
            Arc::new(Package::new("vendor/pkg", "1.0.0.0")),
            Arc::new(Package::new("vendor/pkg", "2.0.0.0")),
            Arc::new(Package::new("vendor/pkg", "1.5.0.0")),
        ];

        let best = find_best_version(&packages, Some("^1.0"), "stable");
        assert!(best.is_some());
        assert_eq!(best.unwrap().version, "1.5.0.0");
    }

    #[test]
    fn test_find_best_version_respects_stability() {
        let mut stable = Package::new("vendor/pkg", "1.0.0.0");
        stable.pretty_version = Some("1.0.0".to_string());

        let mut dev = Package::new("vendor/pkg", "dev-main");
        dev.pretty_version = Some("dev-main".to_string());

        let packages = vec![Arc::new(stable), Arc::new(dev)];

        let best = find_best_version(&packages, None, "stable");
        assert!(best.is_some());
        assert_eq!(best.unwrap().version, "1.0.0.0");
    }

    #[test]
    fn test_find_best_version_allows_dev_with_dev_stability() {
        let mut stable = Package::new("vendor/pkg", "1.0.0.0");
        stable.pretty_version = Some("1.0.0".to_string());

        let mut dev = Package::new("vendor/pkg", "9999999-dev");
        dev.pretty_version = Some("dev-main".to_string());

        let packages = vec![Arc::new(stable), Arc::new(dev)];

        let best = find_best_version(&packages, None, "dev");
        assert!(best.is_some());
        // dev-main should be considered as the "latest"
    }

    #[test]
    fn test_find_best_version_uses_normalized_version() {
        let mut v8 = Package::new("symfony/skeleton", "8.0.99.0");
        v8.pretty_version = Some("v8.0.99".to_string());

        let mut v7 = Package::new("symfony/skeleton", "7.4.99.0");
        v7.pretty_version = Some("v7.4.99".to_string());

        let packages = vec![Arc::new(v7), Arc::new(v8)];

        let best = find_best_version(&packages, None, "stable");
        assert!(best.is_some());
        assert_eq!(best.unwrap().version, "8.0.99.0");
    }
}
