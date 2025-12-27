//! Root package version detection.
//!
//! This module handles detecting the version of the root package (the project itself)
//! based on git branch, branch-alias configuration, and environment variables.
//!
//! The priority order is:
//! 1. COMPOSER_ROOT_VERSION environment variable
//! 2. Explicit version in composer.json
//! 3. Branch alias matching the current git branch
//! 4. Git branch name converted to a dev version

use std::collections::HashMap;
use std::path::Path;

use phpx_semver::VersionParser;

/// Result of root version detection
#[derive(Debug, Clone)]
pub struct RootVersion {
    /// The normalized version string (e.g., "6.7.x-dev", "dev-trunk")
    pub version: String,
    /// The pretty version string for display
    pub pretty_version: String,
    /// How the version was determined
    pub source: RootVersionSource,
}

/// How the root version was determined
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootVersionSource {
    /// From COMPOSER_ROOT_VERSION environment variable
    Environment,
    /// From explicit version field in composer.json
    ComposerJson,
    /// From branch-alias matching the current git branch
    BranchAlias,
    /// From git branch name (converted to dev-* version)
    GitBranch,
    /// Default fallback when nothing else works
    Default,
}

impl std::fmt::Display for RootVersionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RootVersionSource::Environment => write!(f, "COMPOSER_ROOT_VERSION env"),
            RootVersionSource::ComposerJson => write!(f, "composer.json version field"),
            RootVersionSource::BranchAlias => write!(f, "branch-alias"),
            RootVersionSource::GitBranch => write!(f, "git branch"),
            RootVersionSource::Default => write!(f, "default"),
        }
    }
}

/// Detects the root package version.
///
/// Priority order:
/// 1. COMPOSER_ROOT_VERSION environment variable
/// 2. Explicit version in composer.json
/// 3. Branch alias matching the current git branch
/// 4. Git branch name converted to a dev version
/// 5. Default "dev-main"
///
/// # Arguments
/// * `working_dir` - The project root directory (where composer.json is)
/// * `composer_version` - The version field from composer.json (if any)
/// * `branch_aliases` - Branch aliases from extra.branch-alias
pub fn detect_root_version(
    working_dir: &Path,
    composer_version: Option<&str>,
    branch_aliases: &HashMap<String, (String, String)>,
) -> RootVersion {
    // 1. Check COMPOSER_ROOT_VERSION environment variable
    if let Ok(env_version) = std::env::var("COMPOSER_ROOT_VERSION") {
        let env_version = env_version.trim();
        if !env_version.is_empty() {
            let (version, pretty_version) = normalize_version(env_version);
            log::debug!(
                "Root version from COMPOSER_ROOT_VERSION: {} (normalized: {})",
                env_version,
                version
            );
            return RootVersion {
                version,
                pretty_version,
                source: RootVersionSource::Environment,
            };
        }
    }

    // 2. Check explicit version in composer.json
    if let Some(explicit_version) = composer_version {
        let explicit_version = explicit_version.trim();
        if !explicit_version.is_empty() {
            let (version, pretty_version) = normalize_version(explicit_version);
            log::debug!(
                "Root version from composer.json: {} (normalized: {})",
                explicit_version,
                version
            );
            return RootVersion {
                version,
                pretty_version,
                source: RootVersionSource::ComposerJson,
            };
        }
    }

    // 3. Try to get git branch and match against branch-alias
    if let Some(branch) = get_git_branch(working_dir) {
        log::debug!("Current git branch: {}", branch);

        // Normalize the branch to a dev version for matching
        let dev_branch = normalize_branch_to_dev(&branch);
        log::trace!("Normalized branch for alias lookup: {}", dev_branch);

        // Check if there's a branch alias for this branch
        if let Some((alias_normalized, alias_pretty)) = branch_aliases.get(&dev_branch) {
            // Fully normalize the version using the semver parser
            let (version, pretty_version) = normalize_version(alias_normalized);
            log::debug!(
                "Root version from branch-alias: {} -> {} (normalized: {}, pretty: {})",
                dev_branch,
                alias_normalized,
                version,
                pretty_version
            );
            return RootVersion {
                version,
                pretty_version: alias_pretty.clone(),
                source: RootVersionSource::BranchAlias,
            };
        }

        // 4. Use git branch as version
        let (version, pretty_version) = normalize_version(&dev_branch);
        log::debug!(
            "Root version from git branch: {} (normalized: {})",
            branch,
            version
        );
        return RootVersion {
            version,
            pretty_version,
            source: RootVersionSource::GitBranch,
        };
    }

    // 5. Default fallback
    log::debug!("Root version defaulting to dev-main");
    RootVersion {
        version: "dev-main".to_string(),
        pretty_version: "dev-main".to_string(),
        source: RootVersionSource::Default,
    }
}

/// Gets the current git branch name.
///
/// Returns None if:
/// - Not in a git repository
/// - In detached HEAD state
/// - Unable to read git files
pub fn get_git_branch(path: &Path) -> Option<String> {
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

    // Check if it's a symbolic reference (normal branch)
    if let Some(stripped) = head.strip_prefix("ref: refs/heads/") {
        return Some(stripped.to_string());
    }

    // Detached HEAD - no branch name available
    // We could try to find a tag or use the commit hash, but for now return None
    None
}

/// Normalizes a branch name to a dev version string.
///
/// Examples:
/// - "main" -> "dev-main"
/// - "master" -> "dev-master"
/// - "trunk" -> "dev-trunk"
/// - "1.0" -> "1.0.x-dev"
/// - "feature/foo" -> "dev-feature/foo"
fn normalize_branch_to_dev(branch: &str) -> String {
    let branch = branch.trim();

    // Check if it already has dev- prefix
    if branch.starts_with("dev-") {
        return branch.to_string();
    }

    // Check if it looks like a version number
    if branch
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        // Numeric branch like "1.0" or "1.x" -> "1.0.x-dev" or "1.x-dev"
        let cleaned = branch.trim_end_matches(".x");
        return format!("{}.x-dev", cleaned);
    }

    // Regular branch name
    format!("dev-{}", branch)
}

/// Normalizes a version string using the semver parser.
///
/// Returns (normalized_version, pretty_version)
fn normalize_version(version: &str) -> (String, String) {
    let parser = VersionParser::new();

    match parser.normalize(version) {
        Ok(normalized) => (normalized, version.to_string()),
        Err(_) => {
            // If normalization fails, use the original
            (version.to_string(), version.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_branch_to_dev() {
        assert_eq!(normalize_branch_to_dev("main"), "dev-main");
        assert_eq!(normalize_branch_to_dev("master"), "dev-master");
        assert_eq!(normalize_branch_to_dev("trunk"), "dev-trunk");
        assert_eq!(normalize_branch_to_dev("dev-main"), "dev-main");
        assert_eq!(normalize_branch_to_dev("1.0"), "1.0.x-dev");
        assert_eq!(normalize_branch_to_dev("1.x"), "1.x-dev");
        assert_eq!(normalize_branch_to_dev("2.0.x"), "2.0.x-dev");
        assert_eq!(normalize_branch_to_dev("feature/foo"), "dev-feature/foo");
    }

    #[test]
    fn test_detect_root_version_from_env() {
        std::env::set_var("COMPOSER_ROOT_VERSION", "1.2.3");
        let result = detect_root_version(Path::new("/nonexistent"), None, &HashMap::new());
        std::env::remove_var("COMPOSER_ROOT_VERSION");

        assert_eq!(result.source, RootVersionSource::Environment);
        assert_eq!(result.pretty_version, "1.2.3");
    }

    #[test]
    fn test_detect_root_version_from_composer_json() {
        let result = detect_root_version(Path::new("/nonexistent"), Some("2.0.0"), &HashMap::new());

        assert_eq!(result.source, RootVersionSource::ComposerJson);
        assert_eq!(result.pretty_version, "2.0.0");
    }

    #[test]
    fn test_detect_root_version_default() {
        let result = detect_root_version(Path::new("/nonexistent"), None, &HashMap::new());

        assert_eq!(result.source, RootVersionSource::Default);
        assert_eq!(result.version, "dev-main");
    }

    #[test]
    fn test_root_version_source_display() {
        assert_eq!(
            RootVersionSource::Environment.to_string(),
            "COMPOSER_ROOT_VERSION env"
        );
        assert_eq!(
            RootVersionSource::ComposerJson.to_string(),
            "composer.json version field"
        );
        assert_eq!(RootVersionSource::BranchAlias.to_string(), "branch-alias");
        assert_eq!(RootVersionSource::GitBranch.to_string(), "git branch");
        assert_eq!(RootVersionSource::Default.to_string(), "default");
    }

    #[test]
    fn test_branch_alias_matching() {
        // Simulate Shopware's branch-alias setup:
        // "dev-master": "6.7.x-dev", "dev-trunk": "6.7.x-dev"
        let mut branch_aliases = HashMap::new();
        branch_aliases.insert(
            "dev-master".to_string(),
            ("6.7.x-dev".to_string(), "6.7.x-dev".to_string()),
        );
        branch_aliases.insert(
            "dev-trunk".to_string(),
            ("6.7.x-dev".to_string(), "6.7.x-dev".to_string()),
        );

        // Simulate being on "trunk" branch - should match "dev-trunk" alias
        // Since we can't create a real git repo in test, we test the normalization
        let normalized = normalize_branch_to_dev("trunk");
        assert_eq!(normalized, "dev-trunk");

        // Verify the alias lookup would work
        let alias = branch_aliases.get(&normalized);
        assert!(alias.is_some());
        let (version, pretty) = alias.unwrap();
        assert_eq!(version, "6.7.x-dev");
        assert_eq!(pretty, "6.7.x-dev");
    }

    #[test]
    fn test_numeric_branch_normalization() {
        // Test version-like branches (common in release branches)
        assert_eq!(normalize_branch_to_dev("6.7"), "6.7.x-dev");
        assert_eq!(normalize_branch_to_dev("1.0"), "1.0.x-dev");
        assert_eq!(normalize_branch_to_dev("2.x"), "2.x-dev");
    }
}
