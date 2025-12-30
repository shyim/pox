//! Authentication configuration for Composer
//!
//! This module handles loading and managing authentication credentials from:
//! - `~/.composer/auth.json` (global)
//! - `./auth.json` (project-local)
//! - Environment variables (COMPOSER_AUTH)
//!
//! # auth.json format
//!
//! ```json
//! {
//!     "http-basic": {
//!         "example.org": {
//!             "username": "user",
//!             "password": "pass"
//!         }
//!     },
//!     "github-oauth": {
//!         "github.com": "token"
//!     },
//!     "gitlab-oauth": {
//!         "gitlab.com": "token"
//!     },
//!     "gitlab-token": {
//!         "gitlab.com": "token"
//!     },
//!     "bitbucket-oauth": {
//!         "bitbucket.org": {
//!             "consumer-key": "key",
//!             "consumer-secret": "secret"
//!         }
//!     },
//!     "bearer": {
//!         "example.org": "token"
//!     }
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::source::ConfigLoader;
use crate::error::{ComposerError, Result};

/// HTTP Basic authentication credentials
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpBasicCredentials {
    pub username: String,
    pub password: String,
}

/// GitLab token authentication (can be simple token or oauth token)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum GitLabAuth {
    /// Simple private token
    Token(String),
    /// OAuth token with explicit key
    OAuth {
        #[serde(rename = "oauth-token")]
        oauth_token: String,
    },
}

impl GitLabAuth {
    /// Get the token string regardless of format
    pub fn token(&self) -> &str {
        match self {
            GitLabAuth::Token(t) => t,
            GitLabAuth::OAuth { oauth_token } => oauth_token,
        }
    }
}

/// Bitbucket OAuth credentials
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BitbucketOAuthCredentials {
    #[serde(rename = "consumer-key")]
    pub consumer_key: String,
    #[serde(rename = "consumer-secret")]
    pub consumer_secret: String,
}

/// Complete authentication configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    /// HTTP Basic authentication by domain
    #[serde(rename = "http-basic", default, skip_serializing_if = "HashMap::is_empty")]
    pub http_basic: HashMap<String, HttpBasicCredentials>,

    /// Bearer token authentication by domain
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub bearer: HashMap<String, String>,

    /// GitHub OAuth tokens by domain
    #[serde(rename = "github-oauth", default, skip_serializing_if = "HashMap::is_empty")]
    pub github_oauth: HashMap<String, String>,

    /// GitLab OAuth tokens by domain
    #[serde(rename = "gitlab-oauth", default, skip_serializing_if = "HashMap::is_empty")]
    pub gitlab_oauth: HashMap<String, String>,

    /// GitLab private tokens by domain
    #[serde(rename = "gitlab-token", default, skip_serializing_if = "HashMap::is_empty")]
    pub gitlab_token: HashMap<String, GitLabAuth>,

    /// Bitbucket OAuth credentials by domain
    #[serde(rename = "bitbucket-oauth", default, skip_serializing_if = "HashMap::is_empty")]
    pub bitbucket_oauth: HashMap<String, BitbucketOAuthCredentials>,
}

impl AuthConfig {
    /// Create a new empty auth config
    pub fn new() -> Self {
        Self::default()
    }

    /// Load auth config from a file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)
            .map_err(|e| ComposerError::Config(format!("Failed to read {}: {}", path.display(), e)))?;

        Self::from_json(&contents)
    }

    /// Parse auth config from JSON string
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json)
            .map_err(|e| ComposerError::Config(format!("Failed to parse auth.json: {}", e)))
    }

    /// Load auth config from the COMPOSER_AUTH environment variable
    pub fn from_env() -> Result<Option<Self>> {
        match std::env::var("COMPOSER_AUTH") {
            Ok(json) if !json.is_empty() => {
                let config = Self::from_json(&json)?;
                Ok(Some(config))
            }
            _ => Ok(None),
        }
    }

    /// Build complete auth config from all sources
    ///
    /// Priority (highest to lowest):
    /// 1. COMPOSER_AUTH environment variable
    /// 2. Project auth.json (./auth.json)
    /// 3. Global auth.json (~/.composer/auth.json)
    pub fn build<P: AsRef<Path>>(project_dir: Option<P>) -> Result<Self> {
        let loader = ConfigLoader::new(true);
        let mut config = Self::default();

        // 1. Load global auth.json
        let global_auth_path = loader.get_composer_home().join("auth.json");
        if global_auth_path.exists() {
            let global = Self::from_file(&global_auth_path)?;
            config.merge(global);
        }

        // 2. Load project auth.json
        if let Some(project_dir) = project_dir {
            let project_auth_path = project_dir.as_ref().join("auth.json");
            if project_auth_path.exists() {
                let project = Self::from_file(&project_auth_path)?;
                config.merge(project);
            }
        }

        // 3. Load from COMPOSER_AUTH env var (highest priority)
        if let Some(env_config) = Self::from_env()? {
            config.merge(env_config);
        }

        Ok(config)
    }

    /// Merge another auth config into this one (other takes precedence)
    pub fn merge(&mut self, other: AuthConfig) {
        for (domain, creds) in other.http_basic {
            self.http_basic.insert(domain, creds);
        }
        for (domain, token) in other.bearer {
            self.bearer.insert(domain, token);
        }
        for (domain, token) in other.github_oauth {
            self.github_oauth.insert(domain, token);
        }
        for (domain, token) in other.gitlab_oauth {
            self.gitlab_oauth.insert(domain, token);
        }
        for (domain, token) in other.gitlab_token {
            self.gitlab_token.insert(domain, token);
        }
        for (domain, creds) in other.bitbucket_oauth {
            self.bitbucket_oauth.insert(domain, creds);
        }
    }

    /// Save auth config to a file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| ComposerError::Config(format!("Failed to create directory: {}", e)))?;
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| ComposerError::Config(format!("Failed to serialize auth config: {}", e)))?;

        fs::write(path, json)
            .map_err(|e| ComposerError::Config(format!("Failed to write {}: {}", path.display(), e)))?;

        Ok(())
    }

    /// Get the global auth.json path
    pub fn global_path() -> PathBuf {
        let loader = ConfigLoader::new(true);
        loader.get_composer_home().join("auth.json")
    }

    /// Check if config is empty (no credentials stored)
    pub fn is_empty(&self) -> bool {
        self.http_basic.is_empty()
            && self.bearer.is_empty()
            && self.github_oauth.is_empty()
            && self.gitlab_oauth.is_empty()
            && self.gitlab_token.is_empty()
            && self.bitbucket_oauth.is_empty()
    }

    // ============ Lookup Methods ============

    /// Get HTTP Basic credentials for a domain
    pub fn get_http_basic(&self, domain: &str) -> Option<&HttpBasicCredentials> {
        self.http_basic.get(domain)
    }

    /// Get bearer token for a domain
    pub fn get_bearer(&self, domain: &str) -> Option<&str> {
        self.bearer.get(domain).map(|s| s.as_str())
    }

    /// Get GitHub OAuth token for a domain
    pub fn get_github_oauth(&self, domain: &str) -> Option<&str> {
        self.github_oauth.get(domain).map(|s| s.as_str())
    }

    /// Get GitLab OAuth token for a domain
    pub fn get_gitlab_oauth(&self, domain: &str) -> Option<&str> {
        self.gitlab_oauth.get(domain).map(|s| s.as_str())
    }

    /// Get GitLab token for a domain (either private token or oauth)
    pub fn get_gitlab_token(&self, domain: &str) -> Option<&str> {
        // First check gitlab-token, then fall back to gitlab-oauth
        if let Some(auth) = self.gitlab_token.get(domain) {
            return Some(auth.token());
        }
        self.gitlab_oauth.get(domain).map(|s| s.as_str())
    }

    /// Get Bitbucket OAuth credentials for a domain
    pub fn get_bitbucket_oauth(&self, domain: &str) -> Option<&BitbucketOAuthCredentials> {
        self.bitbucket_oauth.get(domain)
    }

    // ============ Setter Methods ============

    /// Set HTTP Basic credentials for a domain
    pub fn set_http_basic(&mut self, domain: impl Into<String>, username: impl Into<String>, password: impl Into<String>) {
        self.http_basic.insert(domain.into(), HttpBasicCredentials {
            username: username.into(),
            password: password.into(),
        });
    }

    /// Set bearer token for a domain
    pub fn set_bearer(&mut self, domain: impl Into<String>, token: impl Into<String>) {
        self.bearer.insert(domain.into(), token.into());
    }

    /// Set GitHub OAuth token for a domain
    pub fn set_github_oauth(&mut self, domain: impl Into<String>, token: impl Into<String>) {
        self.github_oauth.insert(domain.into(), token.into());
    }

    /// Set GitLab OAuth token for a domain
    pub fn set_gitlab_oauth(&mut self, domain: impl Into<String>, token: impl Into<String>) {
        self.gitlab_oauth.insert(domain.into(), token.into());
    }

    /// Set GitLab private token for a domain
    pub fn set_gitlab_token(&mut self, domain: impl Into<String>, token: impl Into<String>) {
        self.gitlab_token.insert(domain.into(), GitLabAuth::Token(token.into()));
    }

    /// Set Bitbucket OAuth credentials for a domain
    pub fn set_bitbucket_oauth(&mut self, domain: impl Into<String>, consumer_key: impl Into<String>, consumer_secret: impl Into<String>) {
        self.bitbucket_oauth.insert(domain.into(), BitbucketOAuthCredentials {
            consumer_key: consumer_key.into(),
            consumer_secret: consumer_secret.into(),
        });
    }

    // ============ Remove Methods ============

    /// Remove HTTP Basic credentials for a domain
    pub fn remove_http_basic(&mut self, domain: &str) -> Option<HttpBasicCredentials> {
        self.http_basic.remove(domain)
    }

    /// Remove bearer token for a domain
    pub fn remove_bearer(&mut self, domain: &str) -> Option<String> {
        self.bearer.remove(domain)
    }

    /// Remove GitHub OAuth token for a domain
    pub fn remove_github_oauth(&mut self, domain: &str) -> Option<String> {
        self.github_oauth.remove(domain)
    }

    /// Remove GitLab OAuth token for a domain
    pub fn remove_gitlab_oauth(&mut self, domain: &str) -> Option<String> {
        self.gitlab_oauth.remove(domain)
    }

    /// Remove GitLab token for a domain
    pub fn remove_gitlab_token(&mut self, domain: &str) -> Option<GitLabAuth> {
        self.gitlab_token.remove(domain)
    }

    /// Remove Bitbucket OAuth credentials for a domain
    pub fn remove_bitbucket_oauth(&mut self, domain: &str) -> Option<BitbucketOAuthCredentials> {
        self.bitbucket_oauth.remove(domain)
    }

    // ============ Domain Matching ============

    /// Find credentials for a URL by extracting and matching the domain
    pub fn find_for_url(&self, url: &str) -> AuthMatch<'_> {
        let domain = extract_domain(url);

        if let Some(creds) = self.get_http_basic(&domain) {
            return AuthMatch::HttpBasic(creds);
        }

        if let Some(token) = self.get_bearer(&domain) {
            return AuthMatch::Bearer(token);
        }

        // Check for GitHub
        if is_github_domain(&domain) || domain.contains("github") {
            if let Some(token) = self.get_github_oauth("github.com").or_else(|| self.get_github_oauth(&domain)) {
                return AuthMatch::GitHubOAuth(token);
            }
        }

        // Check for GitLab
        if is_gitlab_domain(&domain) || domain.contains("gitlab") {
            if let Some(token) = self.get_gitlab_token("gitlab.com").or_else(|| self.get_gitlab_token(&domain)) {
                return AuthMatch::GitLabToken(token);
            }
        }

        // Check for Bitbucket
        if is_bitbucket_domain(&domain) || domain.contains("bitbucket") {
            if let Some(creds) = self.get_bitbucket_oauth("bitbucket.org").or_else(|| self.get_bitbucket_oauth(&domain)) {
                return AuthMatch::BitbucketOAuth(creds);
            }
        }

        AuthMatch::None
    }
}

/// Result of looking up authentication for a URL
#[derive(Debug, Clone)]
pub enum AuthMatch<'a> {
    /// No authentication found
    None,
    /// HTTP Basic authentication
    HttpBasic(&'a HttpBasicCredentials),
    /// Bearer token
    Bearer(&'a str),
    /// GitHub OAuth token
    GitHubOAuth(&'a str),
    /// GitLab token (private or oauth)
    GitLabToken(&'a str),
    /// Bitbucket OAuth credentials
    BitbucketOAuth(&'a BitbucketOAuthCredentials),
}

impl<'a> AuthMatch<'a> {
    /// Check if authentication was found
    pub fn is_some(&self) -> bool {
        !matches!(self, AuthMatch::None)
    }

    /// Check if no authentication was found
    pub fn is_none(&self) -> bool {
        matches!(self, AuthMatch::None)
    }
}

/// Extract domain from a URL
fn extract_domain(url: &str) -> String {
    // Handle git@ style URLs
    if url.starts_with("git@") {
        if let Some(host) = url.strip_prefix("git@") {
            if let Some(colon_pos) = host.find(':') {
                return host[..colon_pos].to_lowercase();
            }
        }
    }

    // Handle standard URLs
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            return host.to_lowercase();
        }
    }

    // Fallback: try to extract from common patterns
    let url = url.to_lowercase();
    if let Some(start) = url.find("://") {
        let rest = &url[start + 3..];
        if let Some(end) = rest.find('/') {
            return rest[..end].to_string();
        }
        return rest.to_string();
    }

    url
}

/// Check if domain is a GitHub domain
fn is_github_domain(domain: &str) -> bool {
    domain == "github.com" || domain == "api.github.com" || domain.ends_with(".github.com")
}

/// Check if domain is a GitLab domain
fn is_gitlab_domain(domain: &str) -> bool {
    domain == "gitlab.com" || domain.ends_with(".gitlab.com")
}

/// Check if domain is a Bitbucket domain
fn is_bitbucket_domain(domain: &str) -> bool {
    domain == "bitbucket.org" || domain == "api.bitbucket.org" || domain.ends_with(".bitbucket.org")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::new();
        assert!(config.is_empty());
    }

    #[test]
    fn test_auth_config_from_json() {
        let json = r#"{
            "http-basic": {
                "example.org": {
                    "username": "user",
                    "password": "pass"
                }
            },
            "github-oauth": {
                "github.com": "ghp_token123"
            },
            "gitlab-token": {
                "gitlab.com": "glpat-token123"
            },
            "bitbucket-oauth": {
                "bitbucket.org": {
                    "consumer-key": "key123",
                    "consumer-secret": "secret456"
                }
            },
            "bearer": {
                "private.repo.org": "bearer_token"
            }
        }"#;

        let config = AuthConfig::from_json(json).unwrap();

        assert!(!config.is_empty());

        let basic = config.get_http_basic("example.org").unwrap();
        assert_eq!(basic.username, "user");
        assert_eq!(basic.password, "pass");

        assert_eq!(config.get_github_oauth("github.com"), Some("ghp_token123"));
        assert_eq!(config.get_gitlab_token("gitlab.com"), Some("glpat-token123"));
        assert_eq!(config.get_bearer("private.repo.org"), Some("bearer_token"));

        let bb = config.get_bitbucket_oauth("bitbucket.org").unwrap();
        assert_eq!(bb.consumer_key, "key123");
        assert_eq!(bb.consumer_secret, "secret456");
    }

    #[test]
    fn test_gitlab_token_formats() {
        // Simple token format
        let json1 = r#"{
            "gitlab-token": {
                "gitlab.com": "simple_token"
            }
        }"#;
        let config1 = AuthConfig::from_json(json1).unwrap();
        assert_eq!(config1.get_gitlab_token("gitlab.com"), Some("simple_token"));

        // OAuth token format
        let json2 = r#"{
            "gitlab-token": {
                "gitlab.com": {
                    "oauth-token": "oauth_token"
                }
            }
        }"#;
        let config2 = AuthConfig::from_json(json2).unwrap();
        assert_eq!(config2.get_gitlab_token("gitlab.com"), Some("oauth_token"));
    }

    #[test]
    fn test_auth_config_merge() {
        let mut config1 = AuthConfig::new();
        config1.set_github_oauth("github.com", "token1");
        config1.set_http_basic("example.org", "user1", "pass1");

        let mut config2 = AuthConfig::new();
        config2.set_github_oauth("github.com", "token2"); // Should override
        config2.set_gitlab_oauth("gitlab.com", "gitlab_token");

        config1.merge(config2);

        assert_eq!(config1.get_github_oauth("github.com"), Some("token2"));
        assert_eq!(config1.get_gitlab_oauth("gitlab.com"), Some("gitlab_token"));
        assert!(config1.get_http_basic("example.org").is_some());
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(extract_domain("https://github.com/owner/repo"), "github.com");
        assert_eq!(extract_domain("https://api.github.com/repos/owner/repo"), "api.github.com");
        assert_eq!(extract_domain("git@github.com:owner/repo.git"), "github.com");
        assert_eq!(extract_domain("https://gitlab.example.com/group/repo"), "gitlab.example.com");
        assert_eq!(extract_domain("https://bitbucket.org/owner/repo"), "bitbucket.org");
    }

    #[test]
    fn test_find_for_url() {
        let mut config = AuthConfig::new();
        config.set_github_oauth("github.com", "gh_token");
        config.set_gitlab_token("gitlab.com", "gl_token");
        config.set_http_basic("private.example.org", "user", "pass");

        // GitHub URL
        let auth = config.find_for_url("https://github.com/owner/repo");
        assert!(matches!(auth, AuthMatch::GitHubOAuth("gh_token")));

        // GitLab URL
        let auth = config.find_for_url("https://gitlab.com/owner/repo");
        assert!(matches!(auth, AuthMatch::GitLabToken("gl_token")));

        // HTTP Basic URL
        let auth = config.find_for_url("https://private.example.org/packages.json");
        assert!(matches!(auth, AuthMatch::HttpBasic(_)));

        // Unknown URL
        let auth = config.find_for_url("https://unknown.org/repo");
        assert!(auth.is_none());
    }

    #[test]
    fn test_setters_and_removers() {
        let mut config = AuthConfig::new();

        config.set_github_oauth("github.com", "token");
        assert_eq!(config.get_github_oauth("github.com"), Some("token"));

        config.remove_github_oauth("github.com");
        assert_eq!(config.get_github_oauth("github.com"), None);

        config.set_bitbucket_oauth("bitbucket.org", "key", "secret");
        let bb = config.get_bitbucket_oauth("bitbucket.org").unwrap();
        assert_eq!(bb.consumer_key, "key");
        assert_eq!(bb.consumer_secret, "secret");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let mut config = AuthConfig::new();
        config.set_github_oauth("github.com", "token");
        config.set_http_basic("example.org", "user", "pass");

        let json = serde_json::to_string(&config).unwrap();
        let parsed = AuthConfig::from_json(&json).unwrap();

        assert_eq!(parsed.get_github_oauth("github.com"), Some("token"));
        let basic = parsed.get_http_basic("example.org").unwrap();
        assert_eq!(basic.username, "user");
    }

    #[test]
    fn test_composer_auth_env_var() {
        // Save current env var if set
        let original = std::env::var("COMPOSER_AUTH").ok();

        // Set test value
        std::env::set_var("COMPOSER_AUTH", r#"{"github-oauth":{"github.com":"env_token"}}"#);

        let result = AuthConfig::from_env().unwrap();
        assert!(result.is_some());

        let config = result.unwrap();
        assert_eq!(config.get_github_oauth("github.com"), Some("env_token"));

        // Restore original env var
        match original {
            Some(val) => std::env::set_var("COMPOSER_AUTH", val),
            None => std::env::remove_var("COMPOSER_AUTH"),
        }
    }

    #[test]
    fn test_composer_auth_env_var_empty() {
        // Save current env var if set
        let original = std::env::var("COMPOSER_AUTH").ok();

        // Set empty value
        std::env::set_var("COMPOSER_AUTH", "");

        let result = AuthConfig::from_env().unwrap();
        assert!(result.is_none());

        // Restore original env var
        match original {
            Some(val) => std::env::set_var("COMPOSER_AUTH", val),
            None => std::env::remove_var("COMPOSER_AUTH"),
        }
    }
}
