//! VCS driver trait and common types.

use std::collections::HashMap;

/// Error type for VCS operations
#[derive(Debug, Clone)]
pub enum VcsDriverError {
    /// Repository not found or inaccessible
    NotFound(String),
    /// Authentication required
    AuthRequired(String),
    /// Network error
    Network(String),
    /// Git command failed
    GitError(String),
    /// Invalid repository format
    InvalidFormat(String),
    /// File not found in repository
    FileNotFound(String),
    /// API rate limit exceeded
    RateLimited(String),
}

impl std::fmt::Display for VcsDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VcsDriverError::NotFound(msg) => write!(f, "Repository not found: {}", msg),
            VcsDriverError::AuthRequired(msg) => write!(f, "Authentication required: {}", msg),
            VcsDriverError::Network(msg) => write!(f, "Network error: {}", msg),
            VcsDriverError::GitError(msg) => write!(f, "Git error: {}", msg),
            VcsDriverError::InvalidFormat(msg) => write!(f, "Invalid format: {}", msg),
            VcsDriverError::FileNotFound(msg) => write!(f, "File not found: {}", msg),
            VcsDriverError::RateLimited(msg) => write!(f, "Rate limited: {}", msg),
        }
    }
}

impl std::error::Error for VcsDriverError {}

/// Information about a VCS reference (tag or branch)
#[derive(Debug, Clone)]
pub struct VcsInfo {
    /// The composer.json content parsed as JSON
    pub composer_json: Option<serde_json::Value>,
    /// Commit identifier (SHA for git)
    pub identifier: String,
    /// Timestamp of the commit
    pub time: Option<String>,
}

/// Trait for VCS drivers
pub trait VcsDriver: Send + Sync {
    /// Get the root/default branch identifier
    fn get_root_identifier(&self) -> Result<String, VcsDriverError>;

    /// Get all tags with their identifiers
    /// Returns a map of tag name -> commit identifier
    fn get_tags(&self) -> Result<HashMap<String, String>, VcsDriverError>;

    /// Get all branches with their identifiers
    /// Returns a map of branch name -> commit identifier
    fn get_branches(&self) -> Result<HashMap<String, String>, VcsDriverError>;

    /// Get composer.json content for a specific identifier
    fn get_composer_information(&self, identifier: &str) -> Result<VcsInfo, VcsDriverError>;

    /// Get file content for a specific identifier
    fn get_file_content(&self, file: &str, identifier: &str) -> Result<String, VcsDriverError>;

    /// Check if the driver supports the given URL
    fn supports(url: &str, deep: bool) -> bool where Self: Sized;

    /// Get the repository URL
    fn get_url(&self) -> &str;

    /// Get the VCS type (git, hg, svn, etc.)
    fn get_vcs_type(&self) -> &str;
}

/// Normalize a version string from a tag
pub fn normalize_tag(tag: &str) -> Option<String> {
    let tag = tag.trim();

    // Strip common prefixes
    let version = tag
        .strip_prefix("release-")
        .or_else(|| tag.strip_prefix("v"))
        .unwrap_or(tag);

    // Basic validation - must start with a digit
    if version.is_empty() || !version.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        return None;
    }

    Some(version.to_string())
}

/// Normalize a branch name to a version
pub fn normalize_branch(branch: &str) -> String {
    let branch = branch.trim();

    // Common branch patterns
    match branch {
        "master" | "main" | "trunk" | "default" => "dev-main".to_string(),
        _ => {
            // Check if it looks like a version number (e.g., "1.0", "2.x")
            if branch.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                format!("{}-dev", branch.replace(".x", ".9999999"))
            } else {
                format!("dev-{}", branch)
            }
        }
    }
}

/// Parse a GitHub URL into owner and repo
pub fn parse_github_url(url: &str) -> Option<(String, String)> {
    // Handle various GitHub URL formats:
    // - https://github.com/owner/repo
    // - https://github.com/owner/repo.git
    // - git@github.com:owner/repo.git
    // - git://github.com/owner/repo.git

    let url = url.trim_end_matches(".git");

    if url.contains("github.com") {
        if let Some(path) = url.split("github.com").nth(1) {
            let path = path.trim_start_matches('/').trim_start_matches(':');
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() >= 2 {
                return Some((parts[0].to_string(), parts[1].to_string()));
            }
        }
    }

    None
}

/// Parse a GitLab URL into domain and project path
#[allow(dead_code)]
pub fn parse_gitlab_url(url: &str) -> Option<(String, String)> {
    // Handle various GitLab URL formats:
    // - https://gitlab.com/owner/repo
    // - https://gitlab.example.com/group/subgroup/repo.git
    // - git@gitlab.com:owner/repo.git

    let url = url.trim_end_matches(".git");

    // Try to extract domain and path
    if url.starts_with("git@") {
        // git@gitlab.com:owner/repo
        let parts: Vec<&str> = url.trim_start_matches("git@").splitn(2, ':').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    } else if url.starts_with("https://") || url.starts_with("http://") {
        // https://gitlab.com/owner/repo
        if let Ok(parsed) = url::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                let path = parsed.path().trim_start_matches('/');
                if !path.is_empty() {
                    return Some((host.to_string(), path.to_string()));
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_tag() {
        assert_eq!(normalize_tag("1.0.0"), Some("1.0.0".to_string()));
        assert_eq!(normalize_tag("v1.0.0"), Some("1.0.0".to_string()));
        assert_eq!(normalize_tag("release-1.0.0"), Some("1.0.0".to_string()));
        assert_eq!(normalize_tag("invalid"), None);
        assert_eq!(normalize_tag(""), None);
    }

    #[test]
    fn test_normalize_branch() {
        assert_eq!(normalize_branch("master"), "dev-main");
        assert_eq!(normalize_branch("main"), "dev-main");
        assert_eq!(normalize_branch("develop"), "dev-develop");
        assert_eq!(normalize_branch("1.0"), "1.0-dev");
        assert_eq!(normalize_branch("2.x"), "2.9999999-dev");
    }

    #[test]
    fn test_parse_github_url() {
        assert_eq!(
            parse_github_url("https://github.com/owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_github_url("https://github.com/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_github_url("git@github.com:owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_github_url("git://github.com/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn test_parse_gitlab_url() {
        assert_eq!(
            parse_gitlab_url("https://gitlab.com/owner/repo"),
            Some(("gitlab.com".to_string(), "owner/repo".to_string()))
        );
        assert_eq!(
            parse_gitlab_url("https://gitlab.example.com/group/subgroup/repo.git"),
            Some(("gitlab.example.com".to_string(), "group/subgroup/repo".to_string()))
        );
        assert_eq!(
            parse_gitlab_url("git@gitlab.com:owner/repo.git"),
            Some(("gitlab.com".to_string(), "owner/repo".to_string()))
        );
    }
}
