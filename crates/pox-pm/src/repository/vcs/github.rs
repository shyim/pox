//! GitHub driver - uses GitHub API for repository access.

use std::collections::HashMap;

use super::driver::{VcsDriver, VcsDriverError, VcsInfo, parse_github_url};
use crate::config::AuthConfig;

/// GitHub driver for GitHub repositories
pub struct GitHubDriver {
    /// Repository URL
    url: String,
    /// GitHub owner
    owner: String,
    /// GitHub repository name
    repo: String,
    /// OAuth token (optional)
    oauth_token: Option<String>,
    /// Cached root identifier
    root_identifier: Option<String>,
    /// Cached default branch (for future use)
    #[allow(dead_code)]
    default_branch: Option<String>,
}

impl GitHubDriver {
    /// Create a new GitHub driver
    pub fn new(url: impl Into<String>) -> Result<Self, VcsDriverError> {
        let url = url.into();

        let (owner, repo) = parse_github_url(&url)
            .ok_or_else(|| VcsDriverError::InvalidFormat(format!("Invalid GitHub URL: {}", url)))?;

        Ok(Self {
            url,
            owner,
            repo,
            oauth_token: None,
            root_identifier: None,
            default_branch: None,
        })
    }

    /// Set OAuth token for authentication
    pub fn with_oauth_token(mut self, token: impl Into<String>) -> Self {
        self.oauth_token = Some(token.into());
        self
    }

    /// Configure authentication from AuthConfig
    pub fn with_auth(mut self, auth: &AuthConfig) -> Self {
        // Try to get token for github.com or the specific domain
        if let Some(token) = auth.get_github_oauth("github.com") {
            self.oauth_token = Some(token.to_string());
        }
        self
    }

    /// Make a GitHub API request using blocking reqwest
    fn api_request(&self, endpoint: &str) -> Result<serde_json::Value, VcsDriverError> {
        let url = format!("https://api.github.com/repos/{}/{}{}", self.owner, self.repo, endpoint);

        let client = reqwest::blocking::Client::new();
        let mut request = client.get(&url);

        // Add authentication if available
        if let Some(ref token) = &self.oauth_token {
            request = request.header("Authorization", format!("token {}", token));
        }

        // Add required headers
        request = request
            .header("Accept", "application/vnd.github.v3+json")
            .header("User-Agent", "phpx-composer");

        let response = request.send()
            .map_err(|e: reqwest::Error| VcsDriverError::Network(e.to_string()))?;

        let status = response.status();

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(VcsDriverError::NotFound(format!("{}/{}", self.owner, self.repo)));
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let body = response.text().unwrap_or_default();
            if body.contains("rate limit") {
                return Err(VcsDriverError::RateLimited("GitHub API rate limit exceeded".to_string()));
            }
            return Err(VcsDriverError::AuthRequired("GitHub authentication required".to_string()));
        }

        if !status.is_success() {
            return Err(VcsDriverError::Network(format!("GitHub API error: {}", status)));
        }

        response.json()
            .map_err(|e| VcsDriverError::InvalidFormat(format!("Invalid JSON response: {}", e)))
    }

    /// Get repository info and cache default branch
    #[allow(dead_code)]
    fn get_repo_info(&mut self) -> Result<(), VcsDriverError> {
        if self.default_branch.is_some() {
            return Ok(());
        }

        let info = self.api_request("")?;

        if let Some(branch) = info.get("default_branch").and_then(|v| v.as_str()) {
            self.default_branch = Some(branch.to_string());
        } else {
            self.default_branch = Some("main".to_string());
        }

        Ok(())
    }

    /// Get file content from GitHub API
    fn get_file_content_api(&self, file: &str, ref_name: &str) -> Result<String, VcsDriverError> {
        let endpoint = format!("/contents/{}?ref={}", file, ref_name);
        let response = self.api_request(&endpoint)?;

        // GitHub returns base64 encoded content
        if let Some(content) = response.get("content").and_then(|v| v.as_str()) {
            // Remove newlines from base64
            let content = content.replace('\n', "");
            let decoded = base64_decode(&content)
                .map_err(|e| VcsDriverError::InvalidFormat(format!("Failed to decode base64: {}", e)))?;
            return Ok(decoded);
        }

        Err(VcsDriverError::FileNotFound(file.to_string()))
    }
}

impl VcsDriver for GitHubDriver {
    fn get_root_identifier(&self) -> Result<String, VcsDriverError> {
        if let Some(ref cached) = self.root_identifier {
            return Ok(cached.clone());
        }

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
        let mut page = 1;

        loop {
            let endpoint = format!("/tags?per_page=100&page={}", page);
            let response = self.api_request(&endpoint)?;

            let items = response.as_array()
                .ok_or_else(|| VcsDriverError::InvalidFormat("Expected array".to_string()))?;

            if items.is_empty() {
                break;
            }

            for item in items {
                if let (Some(name), Some(sha)) = (
                    item.get("name").and_then(|v| v.as_str()),
                    item.get("commit").and_then(|c| c.get("sha")).and_then(|v| v.as_str()),
                ) {
                    tags.insert(name.to_string(), sha.to_string());
                }
            }

            page += 1;

            // Safety limit
            if page > 100 {
                break;
            }
        }

        Ok(tags)
    }

    fn get_branches(&self) -> Result<HashMap<String, String>, VcsDriverError> {
        let mut branches = HashMap::new();
        let mut page = 1;

        loop {
            let endpoint = format!("/branches?per_page=100&page={}", page);
            let response = self.api_request(&endpoint)?;

            let items = response.as_array()
                .ok_or_else(|| VcsDriverError::InvalidFormat("Expected array".to_string()))?;

            if items.is_empty() {
                break;
            }

            for item in items {
                if let (Some(name), Some(sha)) = (
                    item.get("name").and_then(|v| v.as_str()),
                    item.get("commit").and_then(|c| c.get("sha")).and_then(|v| v.as_str()),
                ) {
                    branches.insert(name.to_string(), sha.to_string());
                }
            }

            page += 1;

            // Safety limit
            if page > 100 {
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
        let time = self.api_request(&format!("/commits/{}", identifier))
            .ok()
            .and_then(|info| {
                info.get("commit")
                    .and_then(|c| c.get("committer"))
                    .and_then(|c| c.get("date"))
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
        parse_github_url(url).is_some()
    }

    fn get_url(&self) -> &str {
        &self.url
    }

    fn get_vcs_type(&self) -> &str {
        "git"
    }
}

/// Simple base64 decoder
fn base64_decode(input: &str) -> Result<String, String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn decode_char(c: u8) -> Option<u8> {
        ALPHABET.iter().position(|&x| x == c).map(|p| p as u8)
    }

    let input = input.as_bytes();
    let mut output = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits_collected = 0;

    for &byte in input {
        if byte == b'=' || byte == b'\n' || byte == b'\r' || byte == b' ' {
            continue;
        }

        let value = decode_char(byte)
            .ok_or_else(|| format!("Invalid base64 character: {}", byte as char))?;

        buffer = (buffer << 6) | (value as u32);
        bits_collected += 6;

        if bits_collected >= 8 {
            bits_collected -= 8;
            output.push((buffer >> bits_collected) as u8);
            buffer &= (1 << bits_collected) - 1;
        }
    }

    String::from_utf8(output).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_url() {
        assert!(GitHubDriver::supports("https://github.com/owner/repo", false));
        assert!(GitHubDriver::supports("https://github.com/owner/repo.git", false));
        assert!(GitHubDriver::supports("git@github.com:owner/repo.git", false));
        assert!(!GitHubDriver::supports("https://gitlab.com/owner/repo", false));
    }

    #[test]
    fn test_base64_decode() {
        let encoded = "SGVsbG8gV29ybGQ=";
        let decoded = base64_decode(encoded).unwrap();
        assert_eq!(decoded, "Hello World");
    }

    #[test]
    fn test_base64_decode_multiline() {
        let encoded = "SGVs\nbG8g\nV29y\nbGQ=";
        let decoded = base64_decode(encoded).unwrap();
        assert_eq!(decoded, "Hello World");
    }
}
