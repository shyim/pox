use anyhow::{Context, Result};
use clap::Args;
use indexmap::IndexMap;
use regex::Regex;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use pox_pm::{
    Repository,
    config::Config,
    json::{ComposerJson, ComposerLock},
};

#[derive(Args, Debug)]
pub struct FundArgs {
    /// Output format: text or json
    #[arg(short = 'f', long, default_value = "text")]
    pub format: String,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

pub async fn execute(args: FundArgs) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

    if args.format != "text" && args.format != "json" {
        eprintln!(
            "Error: Unsupported format '{}'. See help for supported formats.",
            args.format
        );
        return Ok(1);
    }

    let json_path = working_dir.join("composer.json");
    let _composer_json: ComposerJson = if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        serde_json::from_str(&content)?
    } else {
        eprintln!("Error: composer.json not found in working directory");
        return Ok(1);
    };

    let config = Config::build(Some(&working_dir), true)?;
    let vendor_dir = working_dir.join(&config.vendor_dir);

    let installed_repo =
        Arc::new(pox_pm::repository::InstalledRepository::new(vendor_dir.clone()));
    installed_repo.load().await.ok();
    let packages = installed_repo.get_packages().await;

    let packages: Vec<Arc<pox_pm::Package>> = if packages.is_empty() {
        let lock_path = working_dir.join("composer.lock");
        if lock_path.exists() {
            let lock_content = std::fs::read_to_string(&lock_path)?;
            let lock: ComposerLock = serde_json::from_str(&lock_content)?;

            let mut locked_packages = lock.packages;
            locked_packages.extend(lock.packages_dev);
            locked_packages
                .into_iter()
                .map(|lp| Arc::new(pox_pm::Package::from(lp)))
                .collect()
        } else {
            eprintln!("No composer.lock file present. Run `pox install` first.");
            return Ok(1);
        }
    } else {
        packages
    };

    let mut fundings: BTreeMap<String, IndexMap<String, Vec<String>>> = BTreeMap::new();
    let github_user_regex = Regex::new(r"^https://github\.com/([^/]+)$").unwrap();

    for package in &packages {
        if package.funding.is_empty() {
            continue;
        }

        let parts: Vec<&str> = package.pretty_name().split('/').collect();
        if parts.len() != 2 {
            continue;
        }
        let vendor = parts[0].to_string();
        let package_name = parts[1].to_string();

        for funding in &package.funding {
            let url = match &funding.url {
                Some(u) if !u.is_empty() => u.clone(),
                _ => continue,
            };

            let url = if let Some(funding_type) = &funding.funding_type {
                if funding_type == "github" {
                    if let Some(caps) = github_user_regex.captures(&url) {
                        format!("https://github.com/sponsors/{}", &caps[1])
                    } else {
                        url
                    }
                } else {
                    url
                }
            } else {
                url
            };

            fundings
                .entry(vendor.clone())
                .or_default()
                .entry(url)
                .or_default()
                .push(package_name.clone());
        }
    }

    if fundings.is_empty() {
        if args.format == "json" {
            println!("{{}}");
        } else {
            println!("No funding links were found in your package dependencies. This doesn't mean they don't need your support!");
        }
        return Ok(0);
    }

    match args.format.as_str() {
        "text" => {
            println!("The following packages were found in your dependencies which publish funding information:");

            for (vendor, links) in &fundings {
                println!();
                println!("\x1b[33m{}\x1b[0m", vendor);

                let mut prev_line: Option<String> = None;
                for (url, packages) in links {
                    let line = format!("  \x1b[32m{}\x1b[0m", packages.join(", "));

                    if prev_line.as_ref() != Some(&line) {
                        println!("{}", line);
                        prev_line = Some(line);
                    }

                    println!("    {}", url);
                }
            }

            println!();
            println!("Please consider following these links and sponsoring the work of package authors!");
            println!("Thank you!");
        }
        "json" => {
            println!("{}", serde_json::to_string_pretty(&fundings)?);
        }
        _ => unreachable!(),
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_github_sponsor_url_transform() {
        let regex = Regex::new(r"^https://github\.com/([^/]+)$").unwrap();

        let url = "https://github.com/symfony";
        if let Some(caps) = regex.captures(url) {
            let sponsor_url = format!("https://github.com/sponsors/{}", &caps[1]);
            assert_eq!(sponsor_url, "https://github.com/sponsors/symfony");
        }
    }

    #[test]
    fn test_github_sponsor_url_no_match() {
        let regex = Regex::new(r"^https://github\.com/([^/]+)$").unwrap();

        assert!(regex.captures("https://github.com/sponsors/symfony").is_none());
        assert!(regex.captures("https://github.com/symfony/symfony").is_none());
    }
}
