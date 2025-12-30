//! HTTP client for Composer package manager operations.
//!
//! This module provides a wrapper around `reqwest` with Composer-specific features:
//! - Automatic retry logic with exponential backoff
//! - Progress tracking for downloads
//! - Custom User-Agent and Accept-Encoding headers
//! - Connection pooling and timeout handling
//! - Proxy and custom CA certificate support
//!
//! # Examples
//!
//! Basic usage:
//! ```no_run
//! use pox_pm::http::HttpClient;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = HttpClient::new()?;
//!
//! // Simple GET request
//! let response = client.get("https://repo.packagist.org/packages.json").await?;
//!
//! // GET and parse JSON
//! #[derive(serde::Deserialize)]
//! struct PackagesJson {
//!     packages: Vec<String>,
//! }
//! let packages: PackagesJson = client.get_json("https://repo.packagist.org/packages.json").await?;
//!
//! // Download a file with progress tracking
//! client.download(
//!     "https://example.com/package.zip",
//!     "/tmp/package.zip".as_ref(),
//!     Some(|downloaded, total| {
//!         println!("Downloaded {}/{} bytes", downloaded, total);
//!     })
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! Custom configuration:
//! ```no_run
//! use pox_pm::http::{HttpClient, HttpClientConfig};
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HttpClientConfig::new()
//!     .with_timeout(Duration::from_secs(60))
//!     .with_max_retries(5)
//!     .with_proxy("http://proxy.example.com:8080".to_string());
//!
//! let client = HttpClient::with_config(config)?;
//! # Ok(())
//! # }
//! ```

use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::config::{AuthConfig, AuthMatch};

const DEFAULT_USER_AGENT: &str = "Composer/2.0 (phpx-pm)";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_RETRIES: u32 = 3;
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("HTTP {status}: {url}")]
    HttpStatus { status: u16, url: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Max retries exceeded for {url}")]
    MaxRetries { url: String },

    #[error("JSON deserialization error: {0}")]
    JsonParse(String),
}

pub struct HttpClient {
    client: Client,
    user_agent: String,
    max_retries: u32,
    retry_delay: Duration,
    auth: Option<Arc<AuthConfig>>,
}

impl HttpClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::with_config(HttpClientConfig::default())
    }

    pub fn with_config(config: HttpClientConfig) -> Result<Self, reqwest::Error> {
        let mut builder = Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .gzip(true)
            .user_agent(&config.user_agent);

        // Add proxy if configured
        if let Some(proxy_url) = &config.proxy {
            let proxy = reqwest::Proxy::all(proxy_url)?;
            builder = builder.proxy(proxy);
        }

        // Add custom CA certificate if configured
        if let Some(cafile) = &config.cafile {
            if let Ok(cert_bytes) = std::fs::read(cafile) {
                if let Ok(cert) = reqwest::Certificate::from_pem(&cert_bytes) {
                    builder = builder.add_root_certificate(cert);
                }
            }
        }

        let client = builder.build()?;

        Ok(Self {
            client,
            user_agent: config.user_agent,
            max_retries: config.max_retries,
            retry_delay: config.retry_delay,
            auth: config.auth.map(Arc::new),
        })
    }

    /// Set authentication configuration
    pub fn with_auth(mut self, auth: AuthConfig) -> Self {
        self.auth = Some(Arc::new(auth));
        self
    }

    /// Set authentication configuration (shared)
    pub fn with_auth_shared(mut self, auth: Arc<AuthConfig>) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Perform GET request with automatic retries
    pub async fn get(&self, url: &str) -> Result<Response, HttpError> {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            match self.execute_get(url).await {
                Ok(response) => {
                    // Check for HTTP errors
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    } else if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                        // Retry on server errors and rate limits
                        last_error = Some(HttpError::HttpStatus {
                            status: status.as_u16(),
                            url: url.to_string(),
                        });
                    } else {
                        // Don't retry on client errors (4xx except 429)
                        return Err(HttpError::HttpStatus {
                            status: status.as_u16(),
                            url: url.to_string(),
                        });
                    }
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }

            // Don't sleep after the last attempt
            if attempt < self.max_retries {
                // Exponential backoff: 1s, 2s, 4s, 8s, etc.
                let delay = self.retry_delay * 2_u32.pow(attempt);
                tokio::time::sleep(delay).await;
            }
        }

        // All retries exhausted
        match last_error {
            Some(e) => Err(e),
            None => Err(HttpError::MaxRetries {
                url: url.to_string(),
            }),
        }
    }

    /// Execute a GET request without retries
    async fn execute_get(&self, url: &str) -> Result<Response, HttpError> {
        let mut request = self
            .client
            .get(url)
            .header("Accept-Encoding", "gzip");

        // Apply authentication if available
        if let Some(ref auth) = self.auth {
            request = self.apply_auth(request, url, auth);
        }

        let response = request.send().await?;
        Ok(response)
    }

    /// Apply authentication to a request based on the URL
    fn apply_auth(&self, request: reqwest::RequestBuilder, url: &str, auth: &AuthConfig) -> reqwest::RequestBuilder {
        match auth.find_for_url(url) {
            AuthMatch::HttpBasic(creds) => {
                request.basic_auth(&creds.username, Some(&creds.password))
            }
            AuthMatch::Bearer(token) => {
                request.bearer_auth(token)
            }
            AuthMatch::GitHubOAuth(token) => {
                // GitHub uses Bearer token in Authorization header
                request.bearer_auth(token)
            }
            AuthMatch::GitLabToken(token) => {
                // GitLab can use either PRIVATE-TOKEN header or Bearer auth
                request.header("PRIVATE-TOKEN", token)
            }
            AuthMatch::BitbucketOAuth(creds) => {
                // Bitbucket OAuth uses the consumer key as username
                request.basic_auth(&creds.consumer_key, Some(&creds.consumer_secret))
            }
            AuthMatch::None => request,
        }
    }

    /// GET JSON and deserialize
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, HttpError> {
        let response = self.get(url).await?;
        let text = response.text().await?;

        serde_json::from_str(&text).map_err(|e| HttpError::JsonParse(e.to_string()))
    }

    /// Download file with progress callback
    pub async fn download<F>(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<F>,
    ) -> Result<(), HttpError>
    where
        F: Fn(u64, u64),
    {
        let response = self.get(url).await?;

        // Get total size from Content-Length header
        let total_size = response.content_length().unwrap_or(0);

        // Create parent directories if they don't exist
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Create the file
        let mut file = File::create(dest).await?;
        let mut downloaded: u64 = 0;

        // Stream the response body
        let mut stream = response.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;

            // Call progress callback if provided
            if let Some(ref callback) = progress {
                callback(downloaded, total_size);
            }
        }

        file.flush().await?;

        Ok(())
    }

    /// Download to memory
    pub async fn download_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        let response = self.get(url).await?;
        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Get the configured user agent
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Get the maximum number of retries
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new().expect("Failed to create default HTTP client")
    }
}

#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub proxy: Option<String>,
    pub cafile: Option<PathBuf>,
    pub user_agent: String,
    pub auth: Option<AuthConfig>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_delay: DEFAULT_RETRY_DELAY,
            proxy: None,
            cafile: None,
            user_agent: DEFAULT_USER_AGENT.to_string(),
            auth: None,
        }
    }
}

impl HttpClientConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = retry_delay;
        self
    }

    pub fn with_proxy(mut self, proxy: String) -> Self {
        self.proxy = Some(proxy);
        self
    }

    pub fn with_cafile(mut self, cafile: PathBuf) -> Self {
        self.cafile = Some(cafile);
        self
    }

    pub fn with_user_agent(mut self, user_agent: String) -> Self {
        self.user_agent = user_agent;
        self
    }

    pub fn with_auth(mut self, auth: AuthConfig) -> Self {
        self.auth = Some(auth);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BitbucketOAuthCredentials, HttpBasicCredentials};

    #[test]
    fn test_config_builder() {
        let config = HttpClientConfig::new()
            .with_timeout(Duration::from_secs(60))
            .with_max_retries(5)
            .with_user_agent("Test/1.0".to_string());

        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.user_agent, "Test/1.0");
    }

    // ============ Authentication Tests ============
    // Based on Composer's AuthHelperTest.php patterns

    #[test]
    fn test_auth_config_with_github_oauth() {
        let mut auth = AuthConfig::default();
        auth.github_oauth.insert("github.com".to_string(), "ghp_token123".to_string());

        let config = HttpClientConfig::new().with_auth(auth);
        assert!(config.auth.is_some());

        let auth = config.auth.as_ref().unwrap();
        assert_eq!(auth.get_github_oauth("github.com"), Some("ghp_token123"));
    }

    #[test]
    fn test_auth_config_with_gitlab_token() {
        let mut auth = AuthConfig::default();
        auth.gitlab_token.insert("gitlab.com".to_string(), crate::config::GitLabAuth::Token("glpat-token".to_string()));

        let config = HttpClientConfig::new().with_auth(auth);
        assert!(config.auth.is_some());

        let auth = config.auth.as_ref().unwrap();
        assert_eq!(auth.get_gitlab_token("gitlab.com"), Some("glpat-token"));
    }

    #[test]
    fn test_auth_config_with_bitbucket_oauth() {
        let mut auth = AuthConfig::default();
        auth.bitbucket_oauth.insert("bitbucket.org".to_string(), BitbucketOAuthCredentials {
            consumer_key: "my_key".to_string(),
            consumer_secret: "my_secret".to_string(),
        });

        let config = HttpClientConfig::new().with_auth(auth);
        let auth = config.auth.as_ref().unwrap();

        let creds = auth.get_bitbucket_oauth("bitbucket.org").unwrap();
        assert_eq!(creds.consumer_key, "my_key");
        assert_eq!(creds.consumer_secret, "my_secret");
    }

    #[test]
    fn test_auth_config_with_http_basic() {
        let mut auth = AuthConfig::default();
        auth.http_basic.insert("private.example.org".to_string(), HttpBasicCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        });

        let config = HttpClientConfig::new().with_auth(auth);
        let auth = config.auth.as_ref().unwrap();

        let creds = auth.get_http_basic("private.example.org").unwrap();
        assert_eq!(creds.username, "user");
        assert_eq!(creds.password, "pass");
    }

    #[test]
    fn test_auth_config_with_bearer() {
        let mut auth = AuthConfig::default();
        auth.bearer.insert("api.example.org".to_string(), "bearer_token_xyz".to_string());

        let config = HttpClientConfig::new().with_auth(auth);
        let auth = config.auth.as_ref().unwrap();

        assert_eq!(auth.get_bearer("api.example.org"), Some("bearer_token_xyz"));
    }

    #[test]
    fn test_auth_find_for_github_url() {
        let mut auth = AuthConfig::default();
        auth.github_oauth.insert("github.com".to_string(), "ghp_token".to_string());

        // Standard GitHub URL
        let result = auth.find_for_url("https://github.com/owner/repo");
        assert!(matches!(result, AuthMatch::GitHubOAuth("ghp_token")));

        // API GitHub URL
        let result = auth.find_for_url("https://api.github.com/repos/owner/repo");
        assert!(matches!(result, AuthMatch::GitHubOAuth("ghp_token")));

        // Raw content URL
        let result = auth.find_for_url("https://raw.githubusercontent.com/owner/repo/main/file.txt");
        assert!(matches!(result, AuthMatch::GitHubOAuth("ghp_token")));
    }

    #[test]
    fn test_auth_find_for_gitlab_url() {
        let mut auth = AuthConfig::default();
        auth.gitlab_token.insert("gitlab.com".to_string(), crate::config::GitLabAuth::Token("glpat-token".to_string()));

        let result = auth.find_for_url("https://gitlab.com/group/project");
        assert!(matches!(result, AuthMatch::GitLabToken("glpat-token")));
    }

    #[test]
    fn test_auth_find_for_bitbucket_url() {
        let mut auth = AuthConfig::default();
        auth.bitbucket_oauth.insert("bitbucket.org".to_string(), BitbucketOAuthCredentials {
            consumer_key: "key".to_string(),
            consumer_secret: "secret".to_string(),
        });

        let result = auth.find_for_url("https://bitbucket.org/owner/repo");
        assert!(matches!(result, AuthMatch::BitbucketOAuth(_)));

        if let AuthMatch::BitbucketOAuth(creds) = result {
            assert_eq!(creds.consumer_key, "key");
        }
    }

    #[test]
    fn test_auth_http_basic_takes_priority() {
        // HTTP Basic should take priority over other auth methods for the same domain
        let mut auth = AuthConfig::default();
        auth.http_basic.insert("github.com".to_string(), HttpBasicCredentials {
            username: "user".to_string(),
            password: "pass".to_string(),
        });
        auth.github_oauth.insert("github.com".to_string(), "ghp_token".to_string());

        let result = auth.find_for_url("https://github.com/owner/repo");
        // HTTP Basic should match first based on find_for_url implementation
        assert!(matches!(result, AuthMatch::HttpBasic(_)));
    }

    #[test]
    fn test_auth_no_match_returns_none() {
        let auth = AuthConfig::default();
        let result = auth.find_for_url("https://unknown.example.org/path");
        assert!(matches!(result, AuthMatch::None));
    }

    #[test]
    fn test_client_with_auth_method() {
        let mut auth = AuthConfig::default();
        auth.github_oauth.insert("github.com".to_string(), "token".to_string());

        let client = HttpClient::new().unwrap().with_auth(auth);
        assert!(client.auth.is_some());
    }

    #[test]
    fn test_client_with_auth_shared() {
        let mut auth = AuthConfig::default();
        auth.github_oauth.insert("github.com".to_string(), "token".to_string());
        let shared = Arc::new(auth);

        let client = HttpClient::new().unwrap().with_auth_shared(Arc::clone(&shared));
        assert!(client.auth.is_some());
    }

    #[test]
    fn test_default_config() {
        let config = HttpClientConfig::default();

        assert_eq!(config.timeout, DEFAULT_TIMEOUT);
        assert_eq!(config.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(config.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(config.retry_delay, DEFAULT_RETRY_DELAY);
        assert_eq!(config.user_agent, DEFAULT_USER_AGENT);
        assert!(config.proxy.is_none());
        assert!(config.cafile.is_none());
    }

    #[tokio::test]
    async fn test_client_creation() {
        let client = HttpClient::new();
        assert!(client.is_ok());

        let client = client.unwrap();
        assert_eq!(client.user_agent(), DEFAULT_USER_AGENT);
        assert_eq!(client.max_retries(), DEFAULT_MAX_RETRIES);
    }

    #[tokio::test]
    async fn test_client_with_config() {
        let config = HttpClientConfig::new()
            .with_timeout(Duration::from_secs(60))
            .with_max_retries(5);

        let client = HttpClient::with_config(config);
        assert!(client.is_ok());

        let client = client.unwrap();
        assert_eq!(client.max_retries(), 5);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_get_request() {
        let client = HttpClient::new().unwrap();
        let response = client.get("https://httpbin.org/get").await;
        assert!(response.is_ok());
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_get_json() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        struct Response {
            url: String,
        }

        let client = HttpClient::new().unwrap();
        let response: Result<Response, _> = client.get_json("https://httpbin.org/get").await;
        assert!(response.is_ok());
        assert_eq!(response.unwrap().url, "https://httpbin.org/get");
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_download_bytes() {
        let client = HttpClient::new().unwrap();
        let bytes = client.download_bytes("https://httpbin.org/bytes/100").await;
        assert!(bytes.is_ok());
        assert_eq!(bytes.unwrap().len(), 100);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_download_file() {
        use tempfile::TempDir;

        let client = HttpClient::new().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let dest = temp_dir.path().join("test_file.bin");

        let result = client
            .download("https://httpbin.org/bytes/100", &dest, None::<fn(u64, u64)>)
            .await;

        assert!(result.is_ok());
        assert!(dest.exists());

        let metadata = tokio::fs::metadata(&dest).await.unwrap();
        assert_eq!(metadata.len(), 100);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_download_file_with_progress() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        use tempfile::TempDir;

        let client = HttpClient::new().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let dest = temp_dir.path().join("test_file.bin");

        let downloaded = Arc::new(AtomicU64::new(0));
        let downloaded_clone = Arc::clone(&downloaded);

        let result = client
            .download(
                "https://httpbin.org/bytes/1000",
                &dest,
                Some(move |bytes, _total| {
                    downloaded_clone.store(bytes, Ordering::SeqCst);
                }),
            )
            .await;

        assert!(result.is_ok());
        assert!(dest.exists());
        assert_eq!(downloaded.load(Ordering::SeqCst), 1000);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_error_404() {
        let client = HttpClient::new().unwrap();
        let response = client.get("https://httpbin.org/status/404").await;
        assert!(response.is_err());

        if let Err(HttpError::HttpStatus { status, .. }) = response {
            assert_eq!(status, 404);
        } else {
            panic!("Expected HttpStatus error");
        }
    }

    // ============ Error Handling Tests ============
    // Based on Composer's HttpDownloaderTest.php patterns

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_error_401_unauthorized() {
        let client = HttpClient::new().unwrap();
        let response = client.get("https://httpbin.org/status/401").await;
        assert!(response.is_err());

        if let Err(HttpError::HttpStatus { status, .. }) = response {
            assert_eq!(status, 401);
        } else {
            panic!("Expected HttpStatus error for 401");
        }
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_error_403_forbidden() {
        let client = HttpClient::new().unwrap();
        let response = client.get("https://httpbin.org/status/403").await;
        assert!(response.is_err());

        if let Err(HttpError::HttpStatus { status, .. }) = response {
            assert_eq!(status, 403);
        } else {
            panic!("Expected HttpStatus error for 403");
        }
    }

    #[test]
    fn test_http_error_display() {
        let err = HttpError::HttpStatus {
            status: 404,
            url: "https://example.com/not-found".to_string(),
        };
        assert_eq!(err.to_string(), "HTTP 404: https://example.com/not-found");

        let err = HttpError::MaxRetries {
            url: "https://example.com/timeout".to_string(),
        };
        assert_eq!(err.to_string(), "Max retries exceeded for https://example.com/timeout");

        let err = HttpError::JsonParse("unexpected token".to_string());
        assert_eq!(err.to_string(), "JSON deserialization error: unexpected token");
    }

    #[test]
    fn test_config_with_proxy() {
        let config = HttpClientConfig::new()
            .with_proxy("http://proxy.example.com:8080".to_string());

        assert_eq!(config.proxy, Some("http://proxy.example.com:8080".to_string()));
    }

    #[test]
    fn test_config_with_cafile() {
        let config = HttpClientConfig::new()
            .with_cafile(PathBuf::from("/path/to/ca.crt"));

        assert_eq!(config.cafile, Some(PathBuf::from("/path/to/ca.crt")));
    }

    #[test]
    fn test_config_with_retry_delay() {
        let config = HttpClientConfig::new()
            .with_retry_delay(Duration::from_millis(500));

        assert_eq!(config.retry_delay, Duration::from_millis(500));
    }

    #[test]
    fn test_config_with_connect_timeout() {
        let config = HttpClientConfig::new()
            .with_connect_timeout(Duration::from_secs(5));

        assert_eq!(config.connect_timeout, Duration::from_secs(5));
    }

    #[test]
    fn test_config_all_builder_methods() {
        let auth = AuthConfig::default();
        let config = HttpClientConfig::new()
            .with_timeout(Duration::from_secs(120))
            .with_connect_timeout(Duration::from_secs(15))
            .with_max_retries(10)
            .with_retry_delay(Duration::from_millis(200))
            .with_proxy("http://proxy:8080".to_string())
            .with_cafile(PathBuf::from("/ca.pem"))
            .with_user_agent("CustomAgent/1.0".to_string())
            .with_auth(auth);

        assert_eq!(config.timeout, Duration::from_secs(120));
        assert_eq!(config.connect_timeout, Duration::from_secs(15));
        assert_eq!(config.max_retries, 10);
        assert_eq!(config.retry_delay, Duration::from_millis(200));
        assert_eq!(config.proxy, Some("http://proxy:8080".to_string()));
        assert_eq!(config.cafile, Some(PathBuf::from("/ca.pem")));
        assert_eq!(config.user_agent, "CustomAgent/1.0");
        assert!(config.auth.is_some());
    }

    #[test]
    fn test_default_client_creation() {
        let client = HttpClient::default();
        assert_eq!(client.user_agent(), DEFAULT_USER_AGENT);
        assert_eq!(client.max_retries(), DEFAULT_MAX_RETRIES);
    }

    // ============ URL and Domain Extraction Tests ============
    // Based on Composer's AuthHelper domain matching

    #[test]
    fn test_github_subdomain_auth() {
        let mut auth = AuthConfig::default();
        auth.github_oauth.insert("github.com".to_string(), "token".to_string());

        // codeload.github.com should match github.com auth
        let result = auth.find_for_url("https://codeload.github.com/owner/repo/zip/refs/heads/main");
        assert!(matches!(result, AuthMatch::GitHubOAuth("token")));
    }

    #[test]
    fn test_gitlab_self_hosted() {
        let mut auth = AuthConfig::default();
        auth.gitlab_token.insert("gitlab.mycompany.com".to_string(),
            crate::config::GitLabAuth::Token("private-token".to_string()));

        let result = auth.find_for_url("https://gitlab.mycompany.com/group/project");
        assert!(matches!(result, AuthMatch::GitLabToken("private-token")));
    }

    #[test]
    fn test_bearer_auth_for_custom_domain() {
        let mut auth = AuthConfig::default();
        auth.bearer.insert("packages.mycompany.com".to_string(), "secret-token".to_string());

        let result = auth.find_for_url("https://packages.mycompany.com/composer/packages.json");
        assert!(matches!(result, AuthMatch::Bearer("secret-token")));
    }

    #[test]
    fn test_http_basic_for_private_packagist() {
        let mut auth = AuthConfig::default();
        auth.http_basic.insert("repo.packagist.com".to_string(), HttpBasicCredentials {
            username: "token".to_string(),
            password: "secret123".to_string(),
        });

        let result = auth.find_for_url("https://repo.packagist.com/myorg/packages.json");
        assert!(matches!(result, AuthMatch::HttpBasic(_)));

        if let AuthMatch::HttpBasic(creds) = result {
            assert_eq!(creds.username, "token");
            assert_eq!(creds.password, "secret123");
        }
    }

    // ============ Retry Behavior Tests (Unit) ============

    #[test]
    fn test_retry_config_zero_retries() {
        let config = HttpClientConfig::new().with_max_retries(0);
        assert_eq!(config.max_retries, 0);

        let client = HttpClient::with_config(config).unwrap();
        assert_eq!(client.max_retries(), 0);
    }

    #[test]
    fn test_retry_config_high_retries() {
        let config = HttpClientConfig::new().with_max_retries(100);
        assert_eq!(config.max_retries, 100);
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        // Verify the exponential backoff formula: delay * 2^attempt
        let base_delay = Duration::from_secs(1);

        // Attempt 0: 1 * 2^0 = 1 second
        assert_eq!(base_delay * 2_u32.pow(0), Duration::from_secs(1));

        // Attempt 1: 1 * 2^1 = 2 seconds
        assert_eq!(base_delay * 2_u32.pow(1), Duration::from_secs(2));

        // Attempt 2: 1 * 2^2 = 4 seconds
        assert_eq!(base_delay * 2_u32.pow(2), Duration::from_secs(4));

        // Attempt 3: 1 * 2^3 = 8 seconds
        assert_eq!(base_delay * 2_u32.pow(3), Duration::from_secs(8));
    }

    // ============ AuthMatch Tests ============

    #[test]
    fn test_auth_match_is_some() {
        let auth = AuthMatch::Bearer("token");
        assert!(auth.is_some());
        assert!(!auth.is_none());
    }

    #[test]
    fn test_auth_match_is_none() {
        let auth = AuthMatch::None;
        assert!(!auth.is_some());
        assert!(auth.is_none());
    }

    #[test]
    fn test_auth_match_variants() {
        // Test all AuthMatch variants are constructable
        let _none = AuthMatch::None;

        let creds = HttpBasicCredentials {
            username: "u".to_string(),
            password: "p".to_string(),
        };
        let _basic = AuthMatch::HttpBasic(&creds);
        let _bearer = AuthMatch::Bearer("token");
        let _github = AuthMatch::GitHubOAuth("ghp_xxx");
        let _gitlab = AuthMatch::GitLabToken("glpat-xxx");

        let bb_creds = BitbucketOAuthCredentials {
            consumer_key: "key".to_string(),
            consumer_secret: "secret".to_string(),
        };
        let _bitbucket = AuthMatch::BitbucketOAuth(&bb_creds);
    }
}
