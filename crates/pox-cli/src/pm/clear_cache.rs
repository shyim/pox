//! Clear-cache command - clear the Composer cache.

use anyhow::{Context, Result};
use clap::Args;
use console::style;
use std::path::PathBuf;

use pox_pm::cache::Cache;
use pox_pm::config::ConfigLoader;

#[derive(Args, Debug)]
pub struct ClearCacheArgs {
    /// Only clear the files cache (downloaded archives)
    #[arg(long)]
    pub files: bool,

    /// Only clear the repo cache (repository metadata)
    #[arg(long)]
    pub repo: bool,

    /// Only clear the VCS cache (cloned repositories)
    #[arg(long)]
    pub vcs: bool,

    /// Run garbage collection instead of full clear (removes stale entries)
    #[arg(long)]
    pub gc: bool,

    /// TTL in seconds for garbage collection (default: 6 months)
    #[arg(long, default_value = "15552000")]
    pub gc_ttl: u64,
}

pub async fn execute(args: ClearCacheArgs) -> Result<i32> {
    let loader = ConfigLoader::new(true);
    let cache_dir = loader.get_cache_dir();

    if !cache_dir.exists() {
        println!("{} Cache directory does not exist: {}",
            style("Info:").cyan(),
            cache_dir.display()
        );
        return Ok(0);
    }

    // Determine which caches to clear
    let clear_all = !args.files && !args.repo && !args.vcs;
    let clear_files = clear_all || args.files;
    let clear_repo = clear_all || args.repo;
    let clear_vcs = clear_all || args.vcs;

    let mut total_freed: u64 = 0;

    if args.gc {
        // Garbage collection mode
        let ttl = std::time::Duration::from_secs(args.gc_ttl);

        println!("{} Running garbage collection (TTL: {} days)...",
            style("Info:").cyan(),
            args.gc_ttl / 86400
        );

        if clear_files {
            let freed = gc_cache_dir(&cache_dir.join("files"), ttl, "files")?;
            total_freed += freed;
        }

        if clear_repo {
            let freed = gc_cache_dir(&cache_dir.join("repo"), ttl, "repo")?;
            total_freed += freed;
        }

        if clear_vcs {
            let freed = gc_vcs_cache(&cache_dir.join("vcs"), ttl)?;
            total_freed += freed;
        }

        println!("\n{} Freed {}",
            style("Success:").green().bold(),
            format_bytes(total_freed)
        );
    } else {
        // Full clear mode
        println!("{} Clearing cache at {}...",
            style("Info:").cyan(),
            cache_dir.display()
        );

        if clear_files {
            let freed = clear_cache_dir(&cache_dir.join("files"), "files")?;
            total_freed += freed;
        }

        if clear_repo {
            let freed = clear_cache_dir(&cache_dir.join("repo"), "repo")?;
            total_freed += freed;
        }

        if clear_vcs {
            let freed = clear_cache_dir(&cache_dir.join("vcs"), "vcs")?;
            total_freed += freed;
        }

        println!("\n{} Cache cleared. Freed {}",
            style("Success:").green().bold(),
            format_bytes(total_freed)
        );
    }

    Ok(0)
}

/// Clear a cache directory completely
fn clear_cache_dir(path: &PathBuf, name: &str) -> Result<u64> {
    if !path.exists() {
        println!("  {} cache: not present", name);
        return Ok(0);
    }

    let cache = Cache::new(path.clone());
    let size = cache.size().context("Failed to calculate cache size")?;

    cache.clear().context(format!("Failed to clear {} cache", name))?;

    println!("  {} cache: cleared ({})", name, format_bytes(size));
    Ok(size)
}

/// Run garbage collection on a cache directory
fn gc_cache_dir(path: &PathBuf, ttl: std::time::Duration, name: &str) -> Result<u64> {
    if !path.exists() {
        println!("  {} cache: not present", name);
        return Ok(0);
    }

    let cache = Cache::new(path.clone());
    let freed = cache.gc(ttl).context(format!("Failed to GC {} cache", name))?;

    if freed > 0 {
        println!("  {} cache: freed {}", name, format_bytes(freed));
    } else {
        println!("  {} cache: nothing to clean", name);
    }

    Ok(freed)
}

/// Run garbage collection on VCS cache (directory-based)
fn gc_vcs_cache(path: &PathBuf, ttl: std::time::Duration) -> Result<u64> {
    if !path.exists() {
        println!("  vcs cache: not present");
        return Ok(0);
    }

    let cache = Cache::new(path.clone());
    let freed = cache.gc_vcs(ttl).context("Failed to GC vcs cache")?;

    if freed > 0 {
        println!("  vcs cache: freed {}", format_bytes(freed));
    } else {
        println!("  vcs cache: nothing to clean");
    }

    Ok(freed)
}

/// Format bytes into human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}
