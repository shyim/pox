use std::sync::Arc;
use async_trait::async_trait;

use crate::package::Package;

/// Search mode for repository searches
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Full text search in name and description
    Fulltext,
    /// Search by package name only
    Name,
    /// Search by vendor only
    Vendor,
}

impl Default for SearchMode {
    fn default() -> Self {
        SearchMode::Fulltext
    }
}

/// Search result from a repository
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Package name (vendor/package)
    pub name: String,
    /// Package description
    pub description: Option<String>,
    /// URL to package page
    pub url: Option<String>,
    /// Abandonment notice
    pub abandoned: Option<String>,
    /// Number of downloads
    pub downloads: Option<u64>,
    /// Number of favorites/stars
    pub favers: Option<u64>,
}

/// Provider information for virtual packages
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    /// Provider package name
    pub name: String,
    /// Provider description
    pub description: Option<String>,
    /// Provider type
    pub package_type: String,
}

/// Repository interface - read-only package source
#[async_trait]
pub trait Repository: Send + Sync {
    /// Get a unique name for this repository
    fn name(&self) -> &str;

    /// Check if the repository contains a package with the given name
    async fn has_package(&self, name: &str) -> bool;

    /// Find all versions of a package by name
    async fn find_packages(&self, name: &str) -> Vec<Arc<Package>>;

    /// Find a specific package version
    async fn find_package(&self, name: &str, version: &str) -> Option<Arc<Package>>;

    /// Find packages matching a version constraint
    async fn find_packages_with_constraint(
        &self,
        name: &str,
        constraint: &str,
    ) -> Vec<Arc<Package>>;

    /// Get all packages in the repository
    async fn get_packages(&self) -> Vec<Arc<Package>>;

    /// Search for packages
    async fn search(&self, query: &str, mode: SearchMode) -> Vec<SearchResult>;

    /// Get packages that provide a virtual package
    async fn get_providers(&self, package_name: &str) -> Vec<ProviderInfo>;

    /// Get the number of packages in the repository
    async fn count(&self) -> usize {
        self.get_packages().await.len()
    }
}

/// Writable repository interface - can add/remove packages
#[async_trait]
pub trait WritableRepository: Repository {
    /// Add a package to the repository
    async fn add_package(&mut self, package: Package);

    /// Remove a package from the repository
    async fn remove_package(&mut self, package: &Package);

    /// Check if this repository has been modified
    fn is_dirty(&self) -> bool;

    /// Persist changes to the repository
    async fn write(&self) -> std::io::Result<()>;
}

/// Configuration for a repository
#[derive(Debug, Clone)]
pub struct RepositoryConfig {
    /// Repository type
    pub repo_type: RepositoryType,
    /// Repository URL or path
    pub url: String,
    /// Additional options
    pub options: RepositoryOptions,
    /// Inline package data (for Package type repositories)
    pub package: Option<serde_json::Value>,
}

/// Repository type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryType {
    /// Composer repository (Packagist-compatible)
    Composer,
    /// VCS repository (auto-detect)
    Vcs,
    /// Git repository
    Git,
    /// GitHub repository
    GitHub,
    /// GitLab repository
    GitLab,
    /// Bitbucket repository
    Bitbucket,
    /// Path repository (local directory)
    Path,
    /// Artifact repository (zip files in a directory)
    Artifact,
    /// Inline package definition
    Package,
}

/// Repository options
#[derive(Debug, Clone, Default)]
pub struct RepositoryOptions {
    /// Use symlinks for path repositories
    pub symlink: Option<bool>,
    /// Use relative symlinks for path repositories
    pub relative: bool,
    /// Reference mode for path repositories: "none", "config", or "auto"
    pub reference: Option<String>,
    /// Version overrides for path repositories
    pub versions: std::collections::HashMap<String, String>,
    /// SSL verification options
    pub ssl_verify: Option<bool>,
    /// Canonical (affects priority)
    pub canonical: bool,
    /// Exclude packages matching these patterns
    pub exclude: Vec<String>,
    /// Only include packages matching these patterns
    pub only: Vec<String>,
}

/// Result of loading packages from a repository
#[derive(Debug)]
pub struct LoadResult {
    /// Packages that were found
    pub packages: Vec<Arc<Package>>,
    /// Package names that were definitively found in this repository
    /// (should not be looked up in lower-priority repositories)
    pub names_found: Vec<String>,
}
