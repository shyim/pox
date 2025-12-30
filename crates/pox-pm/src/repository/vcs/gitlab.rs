//! GitLab driver - uses GitLab API for repository access.

use std::collections::HashMap;

use super::driver::{VcsDriver, VcsDriverError, VcsInfo, parse_gitlab_url};
use crate::config::AuthConfig;

/// GitLab driver for GitLab repositories
pub struct GitLabDriver {
    /// Repository URL
    url: String,
    /// GitLab API host (e.g., "gitlab.com" or self-hosted)
    api_host: String,
    /// Project path (e.g., "owner/repo" or "group/subgroup/repo")
    project_path: String,
    /// URL-encoded project path for API calls
    project_id: String,
    /// Private token (optional)
    private_token: Option<String>,
    /// Cached default branch
    #[allow(dead_code)]
    default_branch: Option<String>,
}

impl GitLabDriver {
    /// Create a new GitLab driver
    pub fn new(url: impl Into<String>) -> Result<Self, VcsDriverError> {
        let url = url.into();

        let (api_host, project_path) = parse_gitlab_url(&url)
            .ok_or_else(|| VcsDriverError::InvalidFormat(format!("Invalid GitLab URL: {}", url)))?;

        // URL-encode the project path for API calls
        let project_id = urlencoding::encode(&project_path).to_string();

        Ok(Self {
            url,
            api_host,
            project_path,
            project_id,
            private_token: None,
            default_branch: None,
        })
    }

    /// Set private token for authentication
    pub fn with_private_token(mut self, token: impl Into<String>) -> Self {
        self.private_token = Some(token.into());
        self
    }

    /// Configure authentication from AuthConfig
    pub fn with_auth(mut self, auth: &AuthConfig) -> Self {
        // Try to get token for the specific domain first, then gitlab.com
        if let Some(token) = auth.get_gitlab_token(&self.api_host) {
            self.private_token = Some(token.to_string());
        } else if let Some(token) = auth.get_gitlab_token("gitlab.com") {
            self.private_token = Some(token.to_string());
        }
        self
    }

    /// Make a GitLab API request using blocking reqwest
    fn api_request(&self, endpoint: &str) -> Result<serde_json::Value, VcsDriverError> {
        let url = format!(
            "https://{}/api/v4/projects/{}{}",
            self.api_host, self.project_id, endpoint
        );

        let client = reqwest::blocking::Client::new();
        let mut request = client.get(&url);

        // Add authentication if available
        if let Some(ref token) = &self.private_token {
            request = request.header("PRIVATE-TOKEN", token.as_str());
        }

        // Add required headers
        request = request
            .header("Accept", "application/json")
            .header("User-Agent", "phpx-composer");

        let response = request.send()
            .map_err(|e: reqwest::Error| VcsDriverError::Network(e.to_string()))?;

        let status = response.status();

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(VcsDriverError::NotFound(self.project_path.clone()));
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(VcsDriverError::AuthRequired("GitLab authentication required".to_string()));
        }

        if !status.is_success() {
            return Err(VcsDriverError::Network(format!("GitLab API error: {}", status)));
        }

        response.json()
            .map_err(|e| VcsDriverError::InvalidFormat(format!("Invalid JSON response: {}", e)))
    }

    /// Get file content from GitLab API
    fn get_file_content_api(&self, file: &str, ref_name: &str) -> Result<String, VcsDriverError> {
        let encoded_file = urlencoding::encode(file);
        let encoded_ref = urlencoding::encode(ref_name);
        let endpoint = format!("/repository/files/{}?ref={}", encoded_file, encoded_ref);
        let response = self.api_request(&endpoint)?;

        // GitLab returns base64 encoded content
        if let Some(content) = response.get("content").and_then(|v| v.as_str()) {
            let decoded = base64_decode(content)
                .map_err(|e| VcsDriverError::InvalidFormat(format!("Failed to decode base64: {}", e)))?;
            return Ok(decoded);
        }

        Err(VcsDriverError::FileNotFound(file.to_string()))
    }
}

impl VcsDriver for GitLabDriver {
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
        let mut page = 1;

        loop {
            let endpoint = format!("/repository/tags?per_page=100&page={}", page);
            let response = self.api_request(&endpoint)?;

            let items = response.as_array()
                .ok_or_else(|| VcsDriverError::InvalidFormat("Expected array".to_string()))?;

            if items.is_empty() {
                break;
            }

            for item in items {
                if let (Some(name), Some(sha)) = (
                    item.get("name").and_then(|v| v.as_str()),
                    item.get("commit").and_then(|c| c.get("id")).and_then(|v| v.as_str()),
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
            let endpoint = format!("/repository/branches?per_page=100&page={}", page);
            let response = self.api_request(&endpoint)?;

            let items = response.as_array()
                .ok_or_else(|| VcsDriverError::InvalidFormat("Expected array".to_string()))?;

            if items.is_empty() {
                break;
            }

            for item in items {
                if let (Some(name), Some(sha)) = (
                    item.get("name").and_then(|v| v.as_str()),
                    item.get("commit").and_then(|c| c.get("id")).and_then(|v| v.as_str()),
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
        let encoded_id = urlencoding::encode(identifier);
        let time = self.api_request(&format!("/repository/commits/{}", encoded_id))
            .ok()
            .and_then(|info| {
                info.get("committed_date")
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
        parse_gitlab_url(url).is_some() && url.to_lowercase().contains("gitlab")
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
    fn test_parse_gitlab_url() {
        assert!(GitLabDriver::supports("https://gitlab.com/owner/repo", false));
        assert!(GitLabDriver::supports("https://gitlab.com/owner/repo.git", false));
        assert!(GitLabDriver::supports("https://gitlab.example.com/group/subgroup/repo", false));
        assert!(!GitLabDriver::supports("https://github.com/owner/repo", false));
    }

    #[test]
    fn test_gitlab_driver_creation() {
        let driver = GitLabDriver::new("https://gitlab.com/owner/repo").unwrap();
        assert_eq!(driver.api_host, "gitlab.com");
        assert_eq!(driver.project_path, "owner/repo");
        assert_eq!(driver.project_id, "owner%2Frepo");
    }

    #[test]
    fn test_gitlab_driver_with_subgroups() {
        let driver = GitLabDriver::new("https://gitlab.com/group/subgroup/repo").unwrap();
        assert_eq!(driver.project_path, "group/subgroup/repo");
        assert_eq!(driver.project_id, "group%2Fsubgroup%2Frepo");
    }

    #[test]
    fn test_base64_decode() {
        let encoded = "SGVsbG8gV29ybGQ=";
        let decoded = base64_decode(encoded).unwrap();
        assert_eq!(decoded, "Hello World");
    }
}
