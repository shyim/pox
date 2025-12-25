use std::sync::Arc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use async_trait::async_trait;
use tokio::sync::RwLock;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::traits::{Repository, SearchMode, SearchResult, ProviderInfo};
use crate::cache::{RepoCache, CacheMetadata};
use crate::config::AuthConfig;
use crate::package::{Package, Dist, Source, Autoload, AutoloadPath};
use phpx_semver::{Constraint, Operator, VersionParser};

/// Default TTL for cached metadata (10 minutes, matching Composer)
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(600);

/// Result from conditional HTTP request
enum FetchResult {
    /// 304 Not Modified - cached data is still valid
    NotModified,
    /// New data received with metadata
    Modified(String, CacheMetadata),
}

/// Custom deserializer that handles the Packagist v2 "__unset" marker.
/// "__unset" means the field was removed in this version.
fn deserialize_maybe_unset<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(Value::String(s)) if s == "__unset" => Ok(None),
        Some(v) => {
            T::deserialize(v).map(Some).map_err(serde::de::Error::custom)
        }
    }
}

/// Deserialize a HashMap that might be "__unset"
fn deserialize_hashmap_maybe_unset<'de, D>(deserializer: D) -> Result<Option<HashMap<String, String>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_maybe_unset(deserializer)
}

/// Composer repository (Packagist-compatible)
pub struct ComposerRepository {
    /// Repository name/identifier
    name: String,
    /// Repository URL
    url: String,
    /// In-memory package cache
    packages: RwLock<HashMap<String, Vec<Arc<Package>>>>,
    /// HTTP client for API requests
    client: reqwest::Client,
    /// File-based cache for HTTP responses
    file_cache: Option<RepoCache>,
    /// Cache TTL
    cache_ttl: Duration,
    /// Authentication configuration
    auth: Option<Arc<AuthConfig>>,
}

impl ComposerRepository {
    /// Create a new Composer repository
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            packages: RwLock::new(HashMap::new()),
            client: reqwest::Client::builder()
                .user_agent("phpx-composer/0.1.0")
                .build()
                .unwrap_or_default(),
            file_cache: None,
            cache_ttl: DEFAULT_CACHE_TTL,
            auth: None,
        }
    }

    /// Create Packagist.org repository
    pub fn packagist() -> Self {
        Self::new("packagist.org", "https://repo.packagist.org")
    }

    /// Create Packagist.org repository with file caching enabled
    pub fn packagist_with_cache(cache_dir: PathBuf) -> Self {
        let mut repo = Self::packagist();
        repo.set_cache_dir(cache_dir);
        repo
    }

    /// Set the cache directory, enabling file-based caching
    pub fn set_cache_dir(&mut self, cache_dir: PathBuf) {
        self.file_cache = Some(RepoCache::new(cache_dir, &self.url));
    }

    /// Set the cache TTL
    pub fn set_cache_ttl(&mut self, ttl: Duration) {
        self.cache_ttl = ttl;
    }

    /// Get the repository URL
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Set authentication configuration
    pub fn set_auth(&mut self, auth: AuthConfig) {
        self.auth = Some(Arc::new(auth));
    }

    /// Apply authentication to a request builder
    fn apply_auth(&self, mut request: reqwest::RequestBuilder, url: &str) -> reqwest::RequestBuilder {
        if let Some(ref auth) = self.auth {
            match auth.find_for_url(url) {
                crate::config::AuthMatch::HttpBasic(creds) => {
                    request = request.basic_auth(&creds.username, Some(&creds.password));
                }
                crate::config::AuthMatch::Bearer(token) => {
                    request = request.bearer_auth(token);
                }
                crate::config::AuthMatch::GitHubOAuth(token) => {
                    request = request.bearer_auth(token);
                }
                crate::config::AuthMatch::GitLabToken(token) => {
                    request = request.header("PRIVATE-TOKEN", token);
                }
                crate::config::AuthMatch::BitbucketOAuth(creds) => {
                    request = request.basic_auth(&creds.consumer_key, Some(&creds.consumer_secret));
                }
                crate::config::AuthMatch::None => {}
            }
        }
        request
    }

    /// Generate cache key for a package
    fn cache_key(package_name: &str) -> String {
        // Convert vendor/package to vendor~package for safe filesystem use
        format!("provider-{}.json", package_name.replace('/', "~"))
    }

    /// Load package metadata from the Packagist v2 API with caching
    async fn load_package_metadata(&self, name: &str) -> Result<Vec<Arc<Package>>, String> {
        // Check in-memory cache first
        {
            let packages = self.packages.read().await;
            if let Some(pkgs) = packages.get(name) {
                return Ok(pkgs.clone());
            }
        }

        let cache_key = Self::cache_key(name);
        let url = format!("{}/p2/{}.json", self.url, name);

        // Try to use file cache with conditional request
        if let Some(ref file_cache) = self.file_cache {
            // Check if we have cached data
            if let Ok(Some((cached_content, metadata))) = file_cache.read(&cache_key) {
                // Check if cache is still fresh (within TTL)
                if let Ok(Some(age)) = file_cache.age(&cache_key) {
                    if age < self.cache_ttl {
                        // Cache is fresh, use it directly
                        if let Ok(result) = self.parse_and_cache_response(name, &cached_content).await {
                            return Ok(result);
                        }
                    }
                }

                // Cache exists but may be stale - try conditional request
                if let Some(last_modified) = &metadata.last_modified {
                    match self.fetch_if_modified(&url, last_modified).await {
                        Ok(FetchResult::NotModified) => {
                            // 304 Not Modified - use cached data
                            if let Ok(result) = self.parse_and_cache_response(name, &cached_content).await {
                                return Ok(result);
                            }
                        }
                        Ok(FetchResult::Modified(body, new_metadata)) => {
                            // New data received - update cache
                            file_cache.write(&cache_key, body.as_bytes(), &new_metadata).ok();
                            if let Ok(result) = self.parse_and_cache_response(name, body.as_bytes()).await {
                                return Ok(result);
                            }
                        }
                        Err(_) => {
                            // Network error - fall back to cached data
                            if let Ok(result) = self.parse_and_cache_response(name, &cached_content).await {
                                return Ok(result);
                            }
                        }
                    }
                }
            }
        }

        // No cache or cache miss - fetch fresh data
        let (body, metadata) = self.fetch_fresh(&url).await?;

        // Store in file cache if available
        if let Some(ref file_cache) = self.file_cache {
            file_cache.write(&cache_key, body.as_bytes(), &metadata).ok();
        }

        self.parse_and_cache_response(name, body.as_bytes()).await
    }

    /// Fetch with If-Modified-Since header
    async fn fetch_if_modified(&self, url: &str, last_modified: &str) -> Result<FetchResult, String> {
        let request = self.client
            .get(url)
            .header("If-Modified-Since", last_modified);
        let request = self.apply_auth(request, url);
        let response = request
            .send()
            .await
            .map_err(|e| format!("Failed to fetch package metadata: {}", e))?;

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(FetchResult::NotModified);
        }

        if !response.status().is_success() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        // Extract Last-Modified header from response
        let new_last_modified = response
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let body = response.text().await
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        let metadata = CacheMetadata {
            last_modified: new_last_modified,
            etag: None,
        };

        Ok(FetchResult::Modified(body, metadata))
    }

    /// Fetch fresh data without conditional headers
    async fn fetch_fresh(&self, url: &str) -> Result<(String, CacheMetadata), String> {
        let request = self.client.get(url);
        let request = self.apply_auth(request, url);
        let response = request
            .send()
            .await
            .map_err(|e| format!("Failed to fetch package metadata: {}", e))?;

        if !response.status().is_success() {
            // Package not found or other error
            return Ok((String::new(), CacheMetadata::default()));
        }

        // Extract Last-Modified header
        let last_modified = response
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let body = response.text().await
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        let metadata = CacheMetadata {
            last_modified,
            etag: None,
        };

        Ok((body, metadata))
    }

    /// Parse response body and cache in memory
    async fn parse_and_cache_response(&self, name: &str, body: &[u8]) -> Result<Vec<Arc<Package>>, String> {
        if body.is_empty() {
            return Ok(Vec::new());
        }

        let data: PackagistResponse = serde_json::from_slice(body)
            .map_err(|e| format!("Failed to parse package metadata: {}", e))?;

        // Convert to Package structs
        let mut result = Vec::new();

        if let Some(versions) = data.packages.get(name) {
            // In Packagist v2 minified format, only the first version has all fields.
            // Subsequent versions inherit from the first one.
            let base = versions.first().cloned();
            for version_data in versions {
                let pkg = self.convert_to_package(name, version_data, base.as_ref());
                result.push(Arc::new(pkg));
            }
        }

        // Cache the results in memory
        {
            let mut packages = self.packages.write().await;
            packages.insert(name.to_string(), result.clone());
        }

        Ok(result)
    }

    /// Convert Packagist version data to a Package
    /// In minified format, fields not present in `data` are inherited from `base`.
    fn convert_to_package(&self, package_name: &str, data: &PackagistVersion, base: Option<&PackagistVersion>) -> Package {
        let mut pkg = Package::new(package_name, &data.version);

        // Helper macro to get field from data or fallback to base
        macro_rules! get_field {
            ($field:ident) => {
                data.$field.clone().or_else(|| base.and_then(|b| b.$field.clone()))
            };
        }

        pkg.description = get_field!(description);
        pkg.homepage = get_field!(homepage);
        pkg.license = get_field!(license).unwrap_or_default();
        pkg.keywords = get_field!(keywords).unwrap_or_default();
        pkg.require = get_field!(require).unwrap_or_default();
        pkg.require_dev = get_field!(require_dev).unwrap_or_default();
        pkg.conflict = get_field!(conflict).unwrap_or_default();
        pkg.provide = get_field!(provide).unwrap_or_default();
        pkg.replace = get_field!(replace).unwrap_or_default();
        pkg.suggest = get_field!(suggest).unwrap_or_default();
        pkg.package_type = get_field!(package_type).unwrap_or_else(|| "library".to_string());
        pkg.bin = get_field!(bin).unwrap_or_default();

        let source = data.source.as_ref().or_else(|| base.and_then(|b| b.source.as_ref()));
        if let Some(source) = source {
            pkg.source = Some(Source::new(
                &source.source_type,
                &source.url,
                &source.reference,
            ));
        }

        let dist = data.dist.as_ref().or_else(|| base.and_then(|b| b.dist.as_ref()));
        if let Some(dist) = dist {
            let mut d = Dist::new(&dist.dist_type, &dist.url);
            if let Some(ref r) = dist.reference {
                d = d.with_reference(r);
            }
            // Only set shasum if it's non-empty (Packagist often returns empty string)
            if let Some(ref s) = dist.shasum {
                if !s.is_empty() {
                    d = d.with_shasum(s);
                }
            }
            pkg.dist = Some(d);
        }

        let authors = data.authors.as_ref().or_else(|| base.and_then(|b| b.authors.as_ref()));
        if let Some(authors) = authors {
            pkg.authors = authors.iter().map(|a| crate::package::Author {
                name: a.name.clone(),
                email: a.email.clone(),
                homepage: a.homepage.clone(),
                role: a.role.clone(),
            }).collect();
        }

        let autoload = data.autoload.as_ref().or_else(|| base.and_then(|b| b.autoload.as_ref()));
        if let Some(al) = autoload {
            pkg.autoload = Some(Self::convert_autoload(al));
        }

        let autoload_dev = data.autoload_dev.as_ref().or_else(|| base.and_then(|b| b.autoload_dev.as_ref()));
        if let Some(al) = autoload_dev {
            pkg.autoload_dev = Some(Self::convert_autoload(al));
        }

        let time = data.time.as_ref().or_else(|| base.and_then(|b| b.time.as_ref()));
        if let Some(t) = time {
            pkg.time = chrono::DateTime::parse_from_rfc3339(t).ok().map(|dt| dt.with_timezone(&chrono::Utc));
        }

        pkg.notification_url = data.notification_url.clone().or_else(|| base.and_then(|b| b.notification_url.clone()));

        let support = data.support.as_ref().or_else(|| base.and_then(|b| b.support.as_ref()));
        if let Some(s) = support {
            pkg.support = Some(crate::package::Support {
                issues: s.issues.clone(),
                forum: s.forum.clone(),
                wiki: s.wiki.clone(),
                source: s.source.clone(),
                email: s.email.clone(),
                irc: s.irc.clone(),
                docs: s.docs.clone(),
                rss: s.rss.clone(),
                chat: s.chat.clone(),
                security: s.security.clone(),
            });
        }

        let funding = data.funding.as_ref().or_else(|| base.and_then(|b| b.funding.as_ref()));
        if let Some(f) = funding {
            pkg.funding = f.iter().map(|pf| crate::package::Funding {
                funding_type: pf.funding_type.clone(),
                url: pf.url.clone(),
            }).collect();
        }

        pkg.extra = data.extra.clone().or_else(|| base.and_then(|b| b.extra.clone()));

        // Replace self.version constraints with actual version
        pkg.replace_self_version();

        pkg
    }

    fn convert_autoload(al: &PackagistAutoload) -> Autoload {
        let mut autoload = Autoload::default();

        for (namespace, paths) in &al.psr4 {
            let path = Self::json_to_autoload_path(paths);
            autoload.psr4.insert(namespace.clone(), path);
        }

        for (namespace, paths) in &al.psr0 {
            let path = Self::json_to_autoload_path(paths);
            autoload.psr0.insert(namespace.clone(), path);
        }

        autoload.classmap = al.classmap.clone();
        autoload.files = al.files.clone();
        autoload.exclude_from_classmap = al.exclude_from_classmap.clone();

        autoload
    }

    fn json_to_autoload_path(value: &serde_json::Value) -> AutoloadPath {
        match value {
            serde_json::Value::String(s) => AutoloadPath::Single(s.clone()),
            serde_json::Value::Array(arr) => {
                let paths: Vec<String> = arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                if paths.len() == 1 {
                    AutoloadPath::Single(paths[0].clone())
                } else {
                    AutoloadPath::Multiple(paths)
                }
            }
            _ => AutoloadPath::Single(String::new()),
        }
    }
}

#[async_trait]
impl Repository for ComposerRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn has_package(&self, name: &str) -> bool {
        !self.find_packages(name).await.is_empty()
    }

    async fn find_packages(&self, name: &str) -> Vec<Arc<Package>> {
        self.load_package_metadata(name).await.unwrap_or_default()
    }

    async fn find_package(&self, name: &str, version: &str) -> Option<Arc<Package>> {
        let packages = self.find_packages(name).await;
        packages.into_iter().find(|p| p.version == version || p.pretty_version.as_deref() == Some(version))
    }

    async fn find_packages_with_constraint(
        &self,
        name: &str,
        constraint: &str,
    ) -> Vec<Arc<Package>> {
        let packages = self.find_packages(name).await;

        // Handle wildcard constraints
        if constraint == "*" || constraint.is_empty() {
            return packages;
        }

        // Parse the constraint
        let parser = VersionParser::new();
        let parsed_constraint = match parser.parse_constraints(constraint) {
            Ok(c) => c,
            Err(_) => return packages, // Be permissive on parse errors
        };

        // Filter packages by constraint
        packages.into_iter()
            .filter(|pkg| {
                // Normalize the package version
                let normalized = parser.normalize(&pkg.version)
                    .unwrap_or_else(|_| pkg.version.clone());

                // Create a version constraint (== normalized_version)
                let version_constraint = match Constraint::new(Operator::Equal, normalized) {
                    Ok(c) => c,
                    Err(_) => return true, // Be permissive
                };

                // Check if the version matches the constraint
                parsed_constraint.matches(&version_constraint)
            })
            .collect()
    }

    async fn get_packages(&self) -> Vec<Arc<Package>> {
        // For Composer repositories, we don't enumerate all packages
        // (there could be millions)
        Vec::new()
    }

    async fn search(&self, query: &str, _mode: SearchMode) -> Vec<SearchResult> {
        // Search using Packagist search API
        let url = format!("{}/search.json?q={}", self.url, urlencoding::encode(query));

        let response = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        if !response.status().is_success() {
            return Vec::new();
        }

        let data: SearchResponse = match response.json().await {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        data.results.into_iter().map(|r| SearchResult {
            name: r.name,
            description: r.description,
            url: r.url,
            abandoned: None,
        }).collect()
    }

    async fn get_providers(&self, _package_name: &str) -> Vec<ProviderInfo> {
        Vec::new()
    }
}

/// Packagist API response for package metadata
#[derive(Debug, Deserialize)]
struct PackagistResponse {
    packages: HashMap<String, Vec<PackagistVersion>>,
}

/// Package version data from Packagist (v2 minified format)
/// In minified format, only the first version has all fields,
/// subsequent versions only contain changed fields.
/// Fields can be set to "__unset" to indicate removal.
#[derive(Debug, Clone, Deserialize)]
struct PackagistVersion {
    version: String,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    homepage: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    license: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    keywords: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    authors: Option<Vec<PackagistAuthor>>,
    #[serde(default, deserialize_with = "deserialize_hashmap_maybe_unset")]
    require: Option<HashMap<String, String>>,
    #[serde(rename = "require-dev", default, deserialize_with = "deserialize_hashmap_maybe_unset")]
    require_dev: Option<HashMap<String, String>>,
    #[serde(default, deserialize_with = "deserialize_hashmap_maybe_unset")]
    conflict: Option<HashMap<String, String>>,
    #[serde(default, deserialize_with = "deserialize_hashmap_maybe_unset")]
    provide: Option<HashMap<String, String>>,
    #[serde(default, deserialize_with = "deserialize_hashmap_maybe_unset")]
    replace: Option<HashMap<String, String>>,
    #[serde(default, deserialize_with = "deserialize_hashmap_maybe_unset")]
    suggest: Option<HashMap<String, String>>,
    #[serde(rename = "type", default, deserialize_with = "deserialize_maybe_unset")]
    package_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    bin: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    source: Option<PackagistSource>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    dist: Option<PackagistDist>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    autoload: Option<PackagistAutoload>,
    #[serde(rename = "autoload-dev", default, deserialize_with = "deserialize_maybe_unset")]
    autoload_dev: Option<PackagistAutoload>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    time: Option<String>,
    #[serde(rename = "notification-url", default, deserialize_with = "deserialize_maybe_unset")]
    notification_url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    support: Option<PackagistSupport>,
    #[serde(default, deserialize_with = "deserialize_maybe_unset")]
    funding: Option<Vec<PackagistFunding>>,
    #[serde(default)]
    extra: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct PackagistAuthor {
    name: Option<String>,
    email: Option<String>,
    homepage: Option<String>,
    role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PackagistSource {
    #[serde(rename = "type")]
    source_type: String,
    url: String,
    reference: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PackagistDist {
    #[serde(rename = "type")]
    dist_type: String,
    url: String,
    reference: Option<String>,
    shasum: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PackagistAutoload {
    #[serde(rename = "psr-4", default)]
    psr4: HashMap<String, serde_json::Value>,
    #[serde(rename = "psr-0", default)]
    psr0: HashMap<String, serde_json::Value>,
    #[serde(default)]
    classmap: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(rename = "exclude-from-classmap", default)]
    exclude_from_classmap: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PackagistSupport {
    #[serde(default)]
    issues: Option<String>,
    #[serde(default)]
    forum: Option<String>,
    #[serde(default)]
    wiki: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    irc: Option<String>,
    #[serde(default)]
    docs: Option<String>,
    #[serde(default)]
    rss: Option<String>,
    #[serde(default)]
    chat: Option<String>,
    #[serde(default)]
    security: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PackagistFunding {
    #[serde(rename = "type")]
    funding_type: Option<String>,
    url: Option<String>,
}

/// Search API response
#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResultItem>,
}

#[derive(Debug, Deserialize)]
struct SearchResultItem {
    name: String,
    description: Option<String>,
    url: Option<String>,
}
