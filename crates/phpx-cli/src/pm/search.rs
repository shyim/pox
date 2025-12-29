//! Search command - search for packages on Packagist.

use anyhow::{Context, Result};
use clap::Args;
use std::path::PathBuf;

use phpx_pm::{
    config::Config,
    json::ComposerJson,
    repository::{ComposerRepository, RepositoryManager, SearchMode},
};

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Search tokens
    #[arg(required = true)]
    pub tokens: Vec<String>,

    /// Search only in package names
    #[arg(short = 'N', long)]
    pub only_name: bool,

    /// Search only for vendor/organization names
    #[arg(short = 'O', long)]
    pub only_vendor: bool,

    /// Search for a specific package type
    #[arg(short = 't', long)]
    pub r#type: Option<String>,

    /// Output format: text or json
    #[arg(short = 'f', long, default_value = "text")]
    pub format: String,

    /// Working directory
    #[arg(short = 'd', long, default_value = ".")]
    pub working_dir: PathBuf,
}

fn is_valid_format(format: &str) -> bool {
    format == "text" || format == "json"
}

fn determine_search_mode(only_name: bool, only_vendor: bool) -> Option<SearchMode> {
    if only_name && only_vendor {
        None
    } else if only_name {
        Some(SearchMode::Name)
    } else if only_vendor {
        Some(SearchMode::Vendor)
    } else {
        Some(SearchMode::Fulltext)
    }
}

fn format_abandoned(abandoned: &Option<String>) -> Option<serde_json::Value> {
    abandoned.as_ref().map(|a| {
        if a.is_empty() {
            serde_json::json!(true)
        } else {
            serde_json::json!(a)
        }
    })
}

fn truncate_description(description: &str, max_len: usize) -> String {
    if description.len() > max_len {
        format!("{}...", &description[..max_len.saturating_sub(3)])
    } else {
        description.to_string()
    }
}

pub async fn execute(args: SearchArgs) -> Result<i32> {
    let working_dir = args
        .working_dir
        .canonicalize()
        .context("Failed to resolve working directory")?;

    if !is_valid_format(&args.format) {
        eprintln!("Unsupported format \"{}\". See help for supported formats.", args.format);
        return Ok(1);
    }

    let mode = match determine_search_mode(args.only_name, args.only_vendor) {
        Some(m) => m,
        None => {
            eprintln!("--only-name and --only-vendor cannot be used together");
            return Ok(1);
        }
    };

    let query = args.tokens.join(" ");

    let config = Config::build(Some(&working_dir), true)?;

    let mut repo_manager = RepositoryManager::new();

    let json_path = working_dir.join("composer.json");
    if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)?;
        let composer_json: ComposerJson = serde_json::from_str(&content)?;

        for repo in composer_json.repositories.as_vec() {
            repo_manager.add_from_json_repository(&repo);
        }
    }

    let packagist = if let Some(cache_dir) = config.cache_dir {
        ComposerRepository::packagist_with_cache(cache_dir.join("repo"))
    } else {
        ComposerRepository::packagist()
    };
    repo_manager.add_repository(std::sync::Arc::new(packagist));

    let results = repo_manager.search(&query, mode).await;

    if results.is_empty() {
        return Ok(0);
    }

    if args.format == "json" {
        let json: Vec<_> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "description": r.description,
                    "url": r.url,
                    "abandoned": format_abandoned(&r.abandoned),
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&json)?);
    } else {
        let terminal_width = terminal_size::terminal_size()
            .map(|(w, _)| w.0 as usize)
            .unwrap_or(80);

        let name_length = results.iter().map(|r| r.name.len()).max().unwrap_or(0) + 1;

        for result in &results {
            let description = result.description.as_deref().unwrap_or("");
            let warning = if result.abandoned.is_some() {
                "! Abandoned ! "
            } else {
                ""
            };

            let remaining = terminal_width.saturating_sub(name_length + warning.len() + 2);
            let description = truncate_description(description, remaining);

            let padding = " ".repeat(name_length.saturating_sub(result.name.len()));
            println!("{}{}{}{}", result.name, padding, warning, description);
        }
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_format() {
        assert!(is_valid_format("text"));
        assert!(is_valid_format("json"));
        assert!(!is_valid_format("xml"));
        assert!(!is_valid_format("test-format"));
        assert!(!is_valid_format(""));
    }

    #[test]
    fn test_determine_search_mode_fulltext() {
        assert_eq!(
            determine_search_mode(false, false),
            Some(SearchMode::Fulltext)
        );
    }

    #[test]
    fn test_determine_search_mode_name_only() {
        assert_eq!(
            determine_search_mode(true, false),
            Some(SearchMode::Name)
        );
    }

    #[test]
    fn test_determine_search_mode_vendor_only() {
        assert_eq!(
            determine_search_mode(false, true),
            Some(SearchMode::Vendor)
        );
    }

    #[test]
    fn test_determine_search_mode_both_flags_invalid() {
        assert_eq!(determine_search_mode(true, true), None);
    }

    #[test]
    fn test_format_abandoned_none() {
        assert_eq!(format_abandoned(&None), None);
    }

    #[test]
    fn test_format_abandoned_empty_string() {
        let abandoned = Some("".to_string());
        assert_eq!(format_abandoned(&abandoned), Some(serde_json::json!(true)));
    }

    #[test]
    fn test_format_abandoned_with_replacement() {
        let abandoned = Some("vendor/replacement-package".to_string());
        assert_eq!(
            format_abandoned(&abandoned),
            Some(serde_json::json!("vendor/replacement-package"))
        );
    }

    #[test]
    fn test_truncate_description_short() {
        assert_eq!(truncate_description("short", 100), "short");
    }

    #[test]
    fn test_truncate_description_exact() {
        assert_eq!(truncate_description("exact", 5), "exact");
    }

    #[test]
    fn test_truncate_description_long() {
        assert_eq!(
            truncate_description("this is a very long description", 20),
            "this is a very lo..."
        );
    }

    #[test]
    fn test_truncate_description_edge_case() {
        assert_eq!(truncate_description("abc", 3), "abc");
        assert_eq!(truncate_description("abcd", 3), "...");
    }
}
