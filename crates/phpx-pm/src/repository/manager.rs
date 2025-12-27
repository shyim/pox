use std::sync::Arc;

use super::traits::{Repository, RepositoryConfig, RepositoryType, SearchMode, SearchResult};
use super::ComposerRepository;
use super::PlatformRepository;
use super::path::{PathRepository, PathRepositoryOptions};
use super::package::PackageRepository;
use super::artifact::ArtifactRepository;
use super::vcs::{VcsRepository, VcsType};
use crate::package::Package;

/// Manages multiple repositories with priority ordering
pub struct RepositoryManager {
    /// Repositories in priority order (first = highest priority)
    repositories: Vec<Arc<dyn Repository>>,
}

impl RepositoryManager {
    /// Create a new repository manager
    pub fn new() -> Self {
        Self {
            repositories: Vec::new(),
        }
    }

    /// Add a repository (will be added with lowest priority)
    pub fn add_repository(&mut self, repo: Arc<dyn Repository>) {
        self.repositories.push(repo);
    }

    /// Insert a repository at a specific position (0 = highest priority)
    pub fn insert_repository(&mut self, index: usize, repo: Arc<dyn Repository>) {
        self.repositories.insert(index, repo);
    }

    /// Get all repositories
    pub fn repositories(&self) -> &[Arc<dyn Repository>] {
        &self.repositories
    }

    /// Find packages by name across all repositories
    pub async fn find_packages(&self, name: &str) -> Vec<Arc<Package>> {
        let mut packages = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for repo in &self.repositories {
            for pkg in repo.find_packages(name).await {
                let key = format!("{}@{}", pkg.name, pkg.version);
                if !seen.contains(&key) {
                    seen.insert(key);
                    packages.push(pkg);
                }
            }
        }

        packages
    }

    /// Find a specific package version
    pub async fn find_package(&self, name: &str, version: &str) -> Option<Arc<Package>> {
        for repo in &self.repositories {
            if let Some(pkg) = repo.find_package(name, version).await {
                return Some(pkg);
            }
        }
        None
    }

    /// Find packages matching a version constraint across all repositories
    pub async fn find_packages_with_constraint(&self, name: &str, constraint: &str) -> Vec<Arc<Package>> {
        let mut packages = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for repo in &self.repositories {
            for pkg in repo.find_packages_with_constraint(name, constraint).await {
                let key = format!("{}@{}", pkg.name, pkg.version);
                if !seen.contains(&key) {
                    seen.insert(key);
                    packages.push(pkg);
                }
            }
        }

        packages
    }

    /// Search across all repositories
    pub async fn search(&self, query: &str, mode: SearchMode) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for repo in &self.repositories {
            for result in repo.search(query, mode).await {
                if !seen.contains(&result.name) {
                    seen.insert(result.name.clone());
                    results.push(result);
                }
            }
        }

        results
    }

    /// Check if any repository has a package
    pub async fn has_package(&self, name: &str) -> bool {
        for repo in &self.repositories {
            if repo.has_package(name).await {
                return true;
            }
        }
        false
    }

    /// Create a repository manager from configuration
    pub async fn from_configs(configs: Vec<RepositoryConfig>) -> Result<Self, String> {
        let mut manager = Self::new();

        for config in configs {
            let repo: Arc<dyn Repository> = match config.repo_type {
                RepositoryType::Composer => {
                    // Composer/Packagist-compatible repository
                    let name = extract_repo_name(&config.url);
                    Arc::new(ComposerRepository::new(name, &config.url))
                }
                RepositoryType::Path => {
                    // Path repository for local packages
                    let options = extract_path_options(&config);
                    Arc::new(PathRepository::new(&config.url, options))
                }
                RepositoryType::Vcs => {
                    Arc::new(VcsRepository::new(&config.url, VcsType::Vcs))
                }
                RepositoryType::Git => {
                    Arc::new(VcsRepository::new(&config.url, VcsType::Git))
                }
                RepositoryType::GitHub => {
                    Arc::new(VcsRepository::new(&config.url, VcsType::GitHub))
                }
                RepositoryType::GitLab => {
                    Arc::new(VcsRepository::new(&config.url, VcsType::GitLab))
                }
                RepositoryType::Bitbucket => {
                    Arc::new(VcsRepository::new(&config.url, VcsType::Bitbucket))
                }
                RepositoryType::Artifact => {
                    // Artifact repository - scans directory for archive files
                    Arc::new(ArtifactRepository::new(&config.url))
                }
                RepositoryType::Package => {
                    // Inline package definitions
                    if let Some(package_data) = &config.package {
                        match PackageRepository::new(package_data) {
                            Ok(repo) => Arc::new(repo),
                            Err(e) => {
                                eprintln!("Warning: Failed to create package repository: {}", e);
                                continue;
                            }
                        }
                    } else {
                        eprintln!("Warning: Package repository missing 'package' field");
                        continue;
                    }
                }
            };

            manager.add_repository(repo);
        }

        Ok(manager)
    }

    /// Create a repository manager with default Packagist and platform repositories
    pub fn with_defaults() -> Self {
        let mut manager = Self::new();

        // Add platform repository (php, ext-*, etc.)
        manager.add_repository(Arc::new(PlatformRepository::default()));

        // Add packagist.org as default repository
        manager.add_repository(Arc::new(ComposerRepository::packagist()));

        manager
    }

    /// Add repositories from composer.json Repository definitions
    ///
    /// This method takes the Repository enum from the JSON schema and creates
    /// the appropriate repository implementations.
    pub fn add_from_json_repository(&mut self, repo: &crate::json::Repository) {
        use crate::json::Repository as JsonRepo;

        let result: Option<Arc<dyn Repository>> = match repo {
            JsonRepo::Composer { url, .. } => {
                let name = extract_repo_name(url);
                Some(Arc::new(ComposerRepository::new(name, url)))
            }
            JsonRepo::Path { url, options } => {
                let path_options = PathRepositoryOptions {
                    symlink: options.symlink,
                    relative: false,
                    reference: "auto".to_string(),
                    versions: std::collections::HashMap::new(),
                };
                Some(Arc::new(PathRepository::new(url, path_options)))
            }
            JsonRepo::Package { package } => {
                match PackageRepository::new(package) {
                    Ok(repo) => Some(Arc::new(repo)),
                    Err(e) => {
                        eprintln!("Warning: Failed to create package repository: {}", e);
                        None
                    }
                }
            }
            JsonRepo::Vcs { url } => {
                Some(Arc::new(VcsRepository::new(url, VcsType::Vcs)))
            }
            JsonRepo::Git { url } => {
                Some(Arc::new(VcsRepository::new(url, VcsType::Git)))
            }
            JsonRepo::GitHub { url } => {
                Some(Arc::new(VcsRepository::new(url, VcsType::GitHub)))
            }
            JsonRepo::GitLab { url } => {
                Some(Arc::new(VcsRepository::new(url, VcsType::GitLab)))
            }
            JsonRepo::Bitbucket { url } => {
                Some(Arc::new(VcsRepository::new(url, VcsType::Bitbucket)))
            }
            JsonRepo::Artifact { url } => {
                Some(Arc::new(ArtifactRepository::new(url)))
            }
            JsonRepo::Disabled(_) => {
                // Disabled repositories are handled separately
                None
            }
        };

        if let Some(repo) = result {
            self.add_repository(repo);
        }
    }

    /// Add multiple repositories from composer.json
    pub fn add_from_json_repositories(&mut self, repos: &[crate::json::Repository]) {
        for repo in repos {
            self.add_from_json_repository(repo);
        }
    }
}

/// Extract a repository name from a URL
fn extract_repo_name(url: &str) -> String {
    // Try to extract host from URL
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            return host.to_string();
        }
    }
    // Fallback to the URL itself
    url.to_string()
}

/// Extract path repository options from config
fn extract_path_options(config: &RepositoryConfig) -> PathRepositoryOptions {
    PathRepositoryOptions {
        symlink: config.options.symlink,
        relative: config.options.relative,
        reference: config.options.reference.clone().unwrap_or_else(|| "auto".to_string()),
        versions: config.options.versions.clone(),
    }
}

impl Default for RepositoryManager {
    fn default() -> Self {
        Self::new()
    }
}
