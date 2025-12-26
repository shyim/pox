//! Autoload generation for PHP packages.
//!
//! This module generates the vendor/autoload.php and related files
//! that enable automatic class loading in PHP.

mod generator;
mod classmap;

pub use generator::{AutoloadGenerator, AutoloadConfig, PackageAutoload, RootPackageInfo};
pub use classmap::ClassMapGenerator;

use std::path::Path;

/// Get the current git HEAD commit hash (short)
pub fn get_head_commit(path: &Path) -> Option<String> {
    let git_dir = path.join(".git");
    if !git_dir.exists() {
        return None;
    }

    let head_path = git_dir.join("HEAD");
    if !head_path.exists() {
        return None;
    }

    let head_content = std::fs::read_to_string(head_path).ok()?;
    let head = head_content.trim();

    if let Some(stripped) = head.strip_prefix("ref: ") {
        // Reference to another file
        let ref_path = git_dir.join(stripped);
        if ref_path.exists() {
            let ref_content = std::fs::read_to_string(ref_path).ok()?;
            return Some(ref_content.trim().chars().take(7).collect());
        }
        
        // Also check packed-refs?
        // For now, simple implementation
    } else {
        // Detached HEAD or hash
        return Some(head.chars().take(7).collect());
    }

    None
}
