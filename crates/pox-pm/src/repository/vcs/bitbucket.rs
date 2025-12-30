//! Bitbucket driver - uses Bitbucket API for repository access.

use std::collections::HashMap;

use super::driver::{VcsDriver, VcsDriverError, VcsInfo};
use crate::config::AuthConfig;

/// Bitbucket driver for Bitbucket repositories
pub struct BitbucketDriver {
    /// Repository URL
    url: String,
    /// Workspace (owner)
    workspace: String,
    /// Repository slug
    repo_slug: String,
    /// OAuth token (optional)
    oauth_token: Option<String>,
    /// App password (optional, alternative to OAuth)
    app_password: Option<(String, String)>, // (username, password)
}

impl BitbucketDriver {
    /// Create a new Bitbucket driver
    pub fn new(url: impl Into<String>) -> Result<Self, VcsDriverError> {
        let url = url.into();

        let (workspace, repo_slug) = parse_bitbucket_url(&url)
            .ok_or_else(|| VcsDriverError::InvalidFormat(format!("Invalid Bitbucket URL: {}", url)))?;

        Ok(Self {
            url,
            workspace,
            repo_slug,
            oauth_token: None,
            app_password: None,
        })
    }

    /// Set OAuth token for authentication
    pub fn with_oauth_token(mut self, token: impl Into<String>) -> Self {
        self.oauth_token = Some(token.into());
        self
    }

    /// Set app password for authentication
    #[allow(dead_code)]
    pub fn with_app_password(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.app_password = Some((username.into(), password.into()));
        self
    }

    /// Configure authentication from AuthConfig
    pub fn with_auth(mut self, auth: &AuthConfig) -> Self {
        // Try to get OAuth credentials for bitbucket.org
        if let Some(creds) = auth.get_bitbucket_oauth("bitbucket.org") {
            self.app_password = Some((creds.consumer_key.clone(), creds.consumer_secret.clone()));
        }
        self
    }

    /// Make a Bitbucket API request using blocking reqwest
    fn api_request(&self, endpoint: &str) -> Result<serde_json::Value, VcsDriverError> {
        let url = format!(
            "https://api.bitbucket.org/2.0/repositories/{}/{}{}",
            self.workspace, self.repo_slug, endpoint
        );

        let client = reqwest::blocking::Client::new();
        let mut request = client.get(&url);

        // Add authentication if available
        if let Some(ref token) = &self.oauth_token {
            request = request.header("Authorization", format!("Bearer {}", token));
        } else if let Some((ref username, ref password)) = &self.app_password {
            request = request.basic_auth(username, Some(password));
        }

        // Add required headers
        request = request
            .header("Accept", "application/json")
            .header("User-Agent", "pox-composer");

        let response = request.send()
            .map_err(|e: reqwest::Error| VcsDriverError::Network(e.to_string()))?;

        let status = response.status();

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(VcsDriverError::NotFound(format!("{}/{}", self.workspace, self.repo_slug)));
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(VcsDriverError::AuthRequired("Bitbucket authentication required".to_string()));
        }

        if !status.is_success() {
            return Err(VcsDriverError::Network(format!("Bitbucket API error: {}", status)));
        }

        response.json()
            .map_err(|e| VcsDriverError::InvalidFormat(format!("Invalid JSON response: {}", e)))
    }

    /// Get file content from Bitbucket API
    fn get_file_content_api(&self, file: &str, ref_name: &str) -> Result<String, VcsDriverError> {
        // Bitbucket uses /src endpoint for file content
        let url = format!(
            "https://api.bitbucket.org/2.0/repositories/{}/{}/src/{}/{}",
            self.workspace, self.repo_slug, ref_name, file
        );

        let client = reqwest::blocking::Client::new();
        let mut request = client.get(&url);

        // Add authentication if available
        if let Some(ref token) = &self.oauth_token {
            request = request.header("Authorization", format!("Bearer {}", token));
        } else if let Some((ref username, ref password)) = &self.app_password {
            request = request.basic_auth(username, Some(password));
        }

        request = request.header("User-Agent", "pox-composer");

        let response = request.send()
            .map_err(|e: reqwest::Error| VcsDriverError::Network(e.to_string()))?;

        let status = response.status();

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(VcsDriverError::FileNotFound(file.to_string()));
        }

        if !status.is_success() {
            return Err(VcsDriverError::Network(format!("Bitbucket API error: {}", status)));
        }

        response.text()
            .map_err(|e| VcsDriverError::Network(format!("Failed to read response: {}", e)))
    }
}

impl VcsDriver for BitbucketDriver {
    fn get_root_identifier(&self) -> Result<String, VcsDriverError> {
        // Get the default branch's HEAD commit
        let branches = self.get_branches()?;

        // Try common default branches
        for branch in &["main", "master", "trunk", "default"] {
            if let Some(sha) = branches.get(*branch) {
                return Ok(sha.clone());
            }
        }

        // Return first branch if no common default found
        branches.values().next()
            .cloned()
            .ok_or_else(|| VcsDriverError::NotFound("No branches found".to_string()))
    }

    fn get_tags(&self) -> Result<HashMap<String, String>, VcsDriverError> {
        let mut tags = HashMap::new();
        let mut next_url: Option<String> = Some(format!(
            "https://api.bitbucket.org/2.0/repositories/{}/{}/refs/tags?pagelen=100",
            self.workspace, self.repo_slug
        ));

        while let Some(url) = next_url.take() {
            let client = reqwest::blocking::Client::new();
            let mut request = client.get(&url);

            // Add authentication if available
            if let Some(ref token) = &self.oauth_token {
                request = request.header("Authorization", format!("Bearer {}", token));
            } else if let Some((ref username, ref password)) = &self.app_password {
                request = request.basic_auth(username, Some(password));
            }

            request = request
                .header("Accept", "application/json")
                .header("User-Agent", "pox-composer");

            let response: serde_json::Value = request.send()
                .map_err(|e: reqwest::Error| VcsDriverError::Network(e.to_string()))?
                .json()
                .map_err(|e| VcsDriverError::InvalidFormat(format!("Invalid JSON: {}", e)))?;

            if let Some(values) = response.get("values").and_then(|v| v.as_array()) {
                for item in values {
                    if let (Some(name), Some(sha)) = (
                        item.get("name").and_then(|v| v.as_str()),
                        item.get("target").and_then(|t| t.get("hash")).and_then(|v| v.as_str()),
                    ) {
                        tags.insert(name.to_string(), sha.to_string());
                    }
                }
            }

            // Check for next page
            next_url = response.get("next")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Safety limit
            if tags.len() > 10000 {
                break;
            }
        }

        Ok(tags)
    }

    fn get_branches(&self) -> Result<HashMap<String, String>, VcsDriverError> {
        let mut branches = HashMap::new();
        let mut next_url: Option<String> = Some(format!(
            "https://api.bitbucket.org/2.0/repositories/{}/{}/refs/branches?pagelen=100",
            self.workspace, self.repo_slug
        ));

        while let Some(url) = next_url.take() {
            let client = reqwest::blocking::Client::new();
            let mut request = client.get(&url);

            // Add authentication if available
            if let Some(ref token) = &self.oauth_token {
                request = request.header("Authorization", format!("Bearer {}", token));
            } else if let Some((ref username, ref password)) = &self.app_password {
                request = request.basic_auth(username, Some(password));
            }

            request = request
                .header("Accept", "application/json")
                .header("User-Agent", "pox-composer");

            let response: serde_json::Value = request.send()
                .map_err(|e: reqwest::Error| VcsDriverError::Network(e.to_string()))?
                .json()
                .map_err(|e| VcsDriverError::InvalidFormat(format!("Invalid JSON: {}", e)))?;

            if let Some(values) = response.get("values").and_then(|v| v.as_array()) {
                for item in values {
                    if let (Some(name), Some(sha)) = (
                        item.get("name").and_then(|v| v.as_str()),
                        item.get("target").and_then(|t| t.get("hash")).and_then(|v| v.as_str()),
                    ) {
                        branches.insert(name.to_string(), sha.to_string());
                    }
                }
            }

            // Check for next page
            next_url = response.get("next")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Safety limit
            if branches.len() > 10000 {
                break;
            }
        }

        Ok(branches)
    }

    fn get_composer_information(&self, identifier: &str) -> Result<VcsInfo, VcsDriverError> {
        let content = self.get_file_content("composer.json", identifier)?;

        let composer_json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| VcsDriverError::InvalidFormat(format!("Invalid JSON: {}", e)))?;

        // Try to get commit info for timestamp
        let time = self.api_request(&format!("/commit/{}", identifier))
            .ok()
            .and_then(|info| {
                info.get("date")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string())
            });

        Ok(VcsInfo {
            composer_json: Some(composer_json),
            identifier: identifier.to_string(),
            time,
        })
    }

    fn get_file_content(&self, file: &str, identifier: &str) -> Result<String, VcsDriverError> {
        self.get_file_content_api(file, identifier)
    }

    fn supports(url: &str, _deep: bool) -> bool {
        parse_bitbucket_url(url).is_some()
    }

    fn get_url(&self) -> &str {
        &self.url
    }

    fn get_vcs_type(&self) -> &str {
        "git"
    }
}

/// Parse a Bitbucket URL into workspace and repo slug
pub fn parse_bitbucket_url(url: &str) -> Option<(String, String)> {
    // Handle various Bitbucket URL formats:
    // - https://bitbucket.org/owner/repo
    // - https://bitbucket.org/owner/repo.git
    // - git@bitbucket.org:owner/repo.git

    let url = url.trim_end_matches(".git");

    if url.contains("bitbucket.org") {
        if url.starts_with("git@") {
            // git@bitbucket.org:owner/repo
            let path = url.trim_start_matches("git@bitbucket.org:");
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() >= 2 {
                return Some((parts[0].to_string(), parts[1].to_string()));
            }
        } else if let Some(path) = url.split("bitbucket.org").nth(1) {
            let path = path.trim_start_matches('/');
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() >= 2 {
                return Some((parts[0].to_string(), parts[1].to_string()));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bitbucket_url() {
        assert_eq!(
            parse_bitbucket_url("https://bitbucket.org/owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_bitbucket_url("https://bitbucket.org/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_bitbucket_url("git@bitbucket.org:owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_bitbucket_url("https://github.com/owner/repo"),
            None
        );
    }

    #[test]
    fn test_bitbucket_driver_creation() {
        let driver = BitbucketDriver::new("https://bitbucket.org/owner/repo").unwrap();
        assert_eq!(driver.workspace, "owner");
        assert_eq!(driver.repo_slug, "repo");
    }

    #[test]
    fn test_bitbucket_supports() {
        assert!(BitbucketDriver::supports("https://bitbucket.org/owner/repo", false));
        assert!(BitbucketDriver::supports("https://bitbucket.org/owner/repo.git", false));
        assert!(BitbucketDriver::supports("git@bitbucket.org:owner/repo.git", false));
        assert!(!BitbucketDriver::supports("https://github.com/owner/repo", false));
        assert!(!BitbucketDriver::supports("https://gitlab.com/owner/repo", false));
    }
}
