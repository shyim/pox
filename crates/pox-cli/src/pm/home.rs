//! Home/Browse command - opens package repository or homepage in browser.

use anyhow::{Context, Result};
use clap::Args;
use std::path::PathBuf;
use std::sync::Arc;

use pox_pm::{
    Repository,
    config::Config,
    json::ComposerJson,
};

#[derive(Args, Debug)]
pub struct HomeArgs {
    /// Package(s) to browse to
    #[arg(value_name = "PACKAGES")]
    pub packages: Vec<String>,

    /// Open the homepage instead of the repository URL
    #[arg(short = 'H', long)]
    pub homepage: bool,

    /// Only show the homepage or repository URL (don't open browser)
    #[arg(short = 's', long)]
    pub show: bool,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

pub async fn execute(args: HomeArgs) -> Result<i32> {
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

    let config = Config::build(Some(&working_dir), true)?;
    let vendor_dir = working_dir.join(&config.vendor_dir);
    let installed_repo = Arc::new(pox_pm::repository::InstalledRepository::new(vendor_dir));
    installed_repo.load().await.ok();
    let installed_packages = installed_repo.get_packages().await;

    let packages_to_browse = if args.packages.is_empty() {
        if let Some(name) = &composer_json.name {
            eprintln!("No package specified, opening homepage for the root package");
            vec![name.clone()]
        } else {
            eprintln!("Error: No package specified and no root package name found");
            return Ok(1);
        }
    } else {
        args.packages.clone()
    };

    let mut return_code = 0;

    for package_name in &packages_to_browse {
        let name_lower = package_name.to_lowercase();

        let is_root = composer_json.name.as_ref()
            .map(|n| n.to_lowercase() == name_lower)
            .unwrap_or(false);

        if is_root {
            let url = if args.homepage {
                composer_json.homepage.clone()
            } else {
                composer_json.support.source.clone()
                    .or_else(|| composer_json.homepage.clone())
            };

            if let Some(url) = url {
                if is_valid_url(&url) {
                    if args.show {
                        println!("{}", url);
                    } else {
                        open_browser(&url);
                    }
                    continue;
                }
            }

            return_code = 1;
            let msg = if args.homepage {
                "Invalid or missing homepage"
            } else {
                "Invalid or missing repository URL"
            };
            eprintln!("{} for {}", msg, package_name);
            continue;
        }

        let package = installed_packages
            .iter()
            .find(|p| p.name.to_lowercase() == name_lower);

        let package = match package {
            Some(p) => p,
            None => {
                return_code = 1;
                eprintln!("Package {} not found", package_name);
                continue;
            }
        };

        if let Some(url) = get_package_url(package, args.homepage) {
            if is_valid_url(&url) {
                if args.show {
                    println!("{}", url);
                } else {
                    open_browser(&url);
                }
                continue;
            }
        }

        return_code = 1;
        let msg = if args.homepage {
            "Invalid or missing homepage"
        } else {
            "Invalid or missing repository URL"
        };
        eprintln!("{} for {}", msg, package_name);
    }

    Ok(return_code)
}

fn get_package_url(package: &pox_pm::Package, use_homepage: bool) -> Option<String> {
    if use_homepage {
        return package.homepage.clone();
    }

    if let Some(support) = &package.support {
        if let Some(source) = &support.source {
            return Some(source.clone());
        }
    }

    if let Some(source) = &package.source {
        return Some(source.url.clone());
    }

    package.homepage.clone()
}

fn is_valid_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .spawn();
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(url)
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        if std::process::Command::new("xdg-open").arg(url).spawn().is_err() {
            eprintln!("No suitable browser opening command found, open yourself: {}", url);
        }
    }
}
