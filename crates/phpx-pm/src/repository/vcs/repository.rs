//! VCS Repository - discovers packages from version control systems.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::driver::{VcsDriver, VcsDriverError, normalize_tag, normalize_branch};
use super::git::GitDriver;
use super::github::GitHubDriver;
use super::gitlab::GitLabDriver;
use super::bitbucket::BitbucketDriver;
use crate::config::AuthConfig;
use crate::package::{Package, Dist, Source, Autoload, AutoloadPath};
use crate::repository::traits::{Repository, SearchMode, SearchResult, ProviderInfo};

/// Type of VCS driver to use
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsType {
    /// Auto-detect driver
    Vcs,
    /// Git driver (uses git command line)
    Git,
    /// GitHub driver (uses GitHub API)
    GitHub,
    /// GitLab driver (uses GitLab API)
    GitLab,
    /// Bitbucket driver
    Bitbucket,
}

impl VcsType {
    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "vcs" => Some(VcsType::Vcs),
            "git" => Some(VcsType::Git),
            "github" => Some(VcsType::GitHub),
            "gitlab" => Some(VcsType::GitLab),
            "bitbucket" => Some(VcsType::Bitbucket),
            _ => None,
        }
    }
}

/// Internal state for VcsRepository (protected by Mutex)
struct VcsRepositoryState {
    /// Discovered packages
    packages: Vec<Arc<Package>>,
    /// Whether packages have been loaded
    loaded: bool,
}

/// VCS repository - discovers packages from version control systems
pub struct VcsRepository {
    /// Repository name
    name: String,
    /// Repository URL
    url: String,
    /// VCS type
    vcs_type: VcsType,
    /// Authentication configuration
    auth: Option<AuthConfig>,
    /// Mutable state
    state: Mutex<VcsRepositoryState>,
}

impl std::fmt::Debug for VcsRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f.debug_struct("VcsRepository")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("vcs_type", &self.vcs_type)
            .field("packages", &state.packages.len())
            .field("loaded", &state.loaded)
            .finish()
    }
}

impl VcsRepository {
    /// Create a new VCS repository
    pub fn new(url: impl Into<String>, vcs_type: VcsType) -> Self {
        let url = url.into();
        let name = format!("vcs ({})", &url);

        Self {
            name,
            url,
            vcs_type,
            auth: None,
            state: Mutex::new(VcsRepositoryState {
                packages: Vec::new(),
                loaded: false,
            }),
        }
    }

    /// Set authentication configuration
    pub fn with_auth(mut self, auth: AuthConfig) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Create appropriate driver for the URL and type
    fn create_driver(&self) -> Result<Box<dyn VcsDriver>, VcsDriverError> {
        let vcs_type = if self.vcs_type == VcsType::Vcs {
            self.detect_vcs_type()
        } else {
            self.vcs_type
        };

        match vcs_type {
            VcsType::GitHub => {
                let mut driver = GitHubDriver::new(&self.url)?;
                if let Some(ref auth) = self.auth {
                    driver = driver.with_auth(auth);
                }
                Ok(Box::new(driver))
            }
            VcsType::GitLab => {
                let mut driver = GitLabDriver::new(&self.url)?;
                if let Some(ref auth) = self.auth {
                    driver = driver.with_auth(auth);
                }
                Ok(Box::new(driver))
            }
            VcsType::Bitbucket => {
                let mut driver = BitbucketDriver::new(&self.url)?;
                if let Some(ref auth) = self.auth {
                    driver = driver.with_auth(auth);
                }
                Ok(Box::new(driver))
            }
            VcsType::Git | VcsType::Vcs => {
                Ok(Box::new(GitDriver::new(&self.url)))
            }
        }
    }

    /// Detect VCS type from URL
    fn detect_vcs_type(&self) -> VcsType {
        let url_lower = self.url.to_lowercase();

        if url_lower.contains("github.com") {
            return VcsType::GitHub;
        }

        if url_lower.contains("gitlab.com") || url_lower.contains("gitlab") {
            return VcsType::GitLab;
        }

        if url_lower.contains("bitbucket.org") {
            return VcsType::Bitbucket;
        }

        VcsType::Git
    }

    /// Load packages from the VCS repository
    fn load_packages(&self) -> Result<(), VcsDriverError> {
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.loaded {
                return Ok(());
            }
        }

        let driver = self.create_driver()?;
        let mut new_packages = Vec::new();

        if let Ok(tags) = driver.get_tags() {
            for (tag, identifier) in tags {
                if let Some(version) = normalize_tag(&tag) {
                    if let Ok(pkg) = self.create_package_from_ref(&*driver, &tag, &identifier, &version, false) {
                        new_packages.push(Arc::new(pkg));
                    }
                }
            }
        }

        if let Ok(branches) = driver.get_branches() {
            for (branch, identifier) in branches {
                let version = normalize_branch(&branch);
                if let Ok(pkg) = self.create_package_from_ref(&*driver, &branch, &identifier, &version, true) {
                    new_packages.push(Arc::new(pkg));
                }
            }
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.packages.extend(new_packages);
        state.loaded = true;
        Ok(())
    }

    /// Create a package from a VCS reference
    fn create_package_from_ref(
        &self,
        driver: &dyn VcsDriver,
        _ref_name: &str,
        identifier: &str,
        version: &str,
        is_dev: bool,
    ) -> Result<Package, VcsDriverError> {
        let info = driver.get_composer_information(identifier)?;

        let json = info.composer_json
            .ok_or_else(|| VcsDriverError::FileNotFound("composer.json".to_string()))?;

        let name = json.get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| VcsDriverError::InvalidFormat("Missing 'name' in composer.json".to_string()))?;

        let final_version = if !is_dev {
            json.get("version")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string())
                .unwrap_or_else(|| version.to_string())
        } else {
            version.to_string()
        };

        let mut pkg = Package::new(name, &final_version);

        pkg.source = Some(Source::new(
            driver.get_vcs_type(),
            driver.get_url(),
            identifier,
        ));

        if self.detect_vcs_type() == VcsType::GitHub {
            if let Some((owner, repo)) = super::driver::parse_github_url(&self.url) {
                let dist_url = format!(
                    "https://api.github.com/repos/{}/{}/zipball/{}",
                    owner, repo, identifier
                );
                pkg.dist = Some(Dist::new("zip", &dist_url).with_reference(identifier));
            }
        }

        if let Some(time_str) = info.time {
            if let Ok(time) = DateTime::parse_from_rfc3339(&time_str) {
                pkg.time = Some(time.with_timezone(&Utc));
            }
        }

        if let Some(desc) = json.get("description").and_then(|v| v.as_str()) {
            pkg.description = Some(desc.to_string());
        }

        if let Some(t) = json.get("type").and_then(|v| v.as_str()) {
            pkg.package_type = t.to_string();
        }

        if let Some(license) = json.get("license") {
            pkg.license = parse_license(license);
        }

        if let Some(require) = json.get("require").and_then(|v| v.as_object()) {
            pkg.require = require
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("*").to_string()))
                .collect();
        }

        if let Some(require_dev) = json.get("require-dev").and_then(|v| v.as_object()) {
            pkg.require_dev = require_dev
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("*").to_string()))
                .collect();
        }

        if let Some(autoload) = json.get("autoload") {
            pkg.autoload = Some(parse_autoload(autoload));
        }

        if let Some(autoload_dev) = json.get("autoload-dev") {
            pkg.autoload_dev = Some(parse_autoload(autoload_dev));
        }

        if let Some(bin) = json.get("bin").and_then(|v| v.as_array()) {
            pkg.bin = bin
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }

        // Replace self.version constraints with actual version
        pkg.replace_self_version();

        Ok(pkg)
    }
}

#[async_trait]
impl Repository for VcsRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn has_package(&self, name: &str) -> bool {
        !self.find_packages(name).await.is_empty()
    }

    async fn find_packages(&self, name: &str) -> Vec<Arc<Package>> {
        self.load_packages().ok();

        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.packages
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .cloned()
            .collect()
    }

    async fn find_package(&self, name: &str, version: &str) -> Option<Arc<Package>> {
        let packages = self.find_packages(name).await;
        packages.into_iter().find(|p| p.version == version)
    }

    async fn find_packages_with_constraint(
        &self,
        name: &str,
        _constraint: &str,
    ) -> Vec<Arc<Package>> {
        self.find_packages(name).await
    }

    async fn get_packages(&self) -> Vec<Arc<Package>> {
        self.load_packages().ok();

        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.packages.clone()
    }

    async fn search(&self, query: &str, _mode: SearchMode) -> Vec<SearchResult> {
        let packages = self.get_packages().await;

        packages
            .iter()
            .filter(|p| {
                p.name.contains(query) ||
                p.description.as_ref().map(|d| d.contains(query)).unwrap_or(false)
            })
            .map(|p| SearchResult {
                name: p.name.clone(),
                description: p.description.clone(),
                url: Some(self.url.clone()),
                abandoned: None,
                downloads: None,
                favers: None,
            })
            .collect()
    }

    async fn get_providers(&self, _package_name: &str) -> Vec<ProviderInfo> {
        Vec::new()
    }
}

/// Parse license from JSON value
fn parse_license(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse autoload from JSON value
fn parse_autoload(value: &serde_json::Value) -> Autoload {
    let mut autoload = Autoload::default();

    if let Some(psr4) = value.get("psr-4").and_then(|v| v.as_object()) {
        for (namespace, paths) in psr4 {
            let path = json_to_autoload_path(paths);
            autoload.psr4.insert(namespace.clone(), path);
        }
    }

    if let Some(psr0) = value.get("psr-0").and_then(|v| v.as_object()) {
        for (namespace, paths) in psr0 {
            let path = json_to_autoload_path(paths);
            autoload.psr0.insert(namespace.clone(), path);
        }
    }

    if let Some(classmap) = value.get("classmap").and_then(|v| v.as_array()) {
        autoload.classmap = classmap
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }

    if let Some(files) = value.get("files").and_then(|v| v.as_array()) {
        autoload.files = files
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }

    autoload
}

/// Convert JSON value to AutoloadPath
fn json_to_autoload_path(value: &serde_json::Value) -> AutoloadPath {
    match value {
        serde_json::Value::String(s) => AutoloadPath::Single(s.clone()),
        serde_json::Value::Array(arr) => {
            let paths: Vec<String> = arr
                .iter()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vcs_type_from_str() {
        assert_eq!(VcsType::from_str("vcs"), Some(VcsType::Vcs));
        assert_eq!(VcsType::from_str("git"), Some(VcsType::Git));
        assert_eq!(VcsType::from_str("github"), Some(VcsType::GitHub));
        assert_eq!(VcsType::from_str("gitlab"), Some(VcsType::GitLab));
        assert_eq!(VcsType::from_str("bitbucket"), Some(VcsType::Bitbucket));
        assert_eq!(VcsType::from_str("unknown"), None);
    }

    #[test]
    fn test_detect_vcs_type() {
        let repo = VcsRepository::new("https://github.com/owner/repo", VcsType::Vcs);
        assert_eq!(repo.detect_vcs_type(), VcsType::GitHub);

        let repo = VcsRepository::new("https://gitlab.com/owner/repo", VcsType::Vcs);
        assert_eq!(repo.detect_vcs_type(), VcsType::GitLab);

        let repo = VcsRepository::new("https://bitbucket.org/owner/repo", VcsType::Vcs);
        assert_eq!(repo.detect_vcs_type(), VcsType::Bitbucket);

        let repo = VcsRepository::new("https://example.com/repo.git", VcsType::Vcs);
        assert_eq!(repo.detect_vcs_type(), VcsType::Git);
    }
}
