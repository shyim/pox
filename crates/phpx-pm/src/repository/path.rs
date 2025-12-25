//! Path repository - loads packages from local filesystem paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use glob::glob;
use sha1::{Sha1, Digest};
use tokio::sync::RwLock;

use super::traits::{Repository, SearchMode, SearchResult, ProviderInfo};
use crate::package::{Package, Dist, Source, Autoload, AutoloadPath};

/// Options for path repository
#[derive(Debug, Clone, Default)]
pub struct PathRepositoryOptions {
    /// Force symlink (true) or mirror (false), or auto (None)
    pub symlink: Option<bool>,
    /// Keep paths as relative
    pub relative: bool,
    /// Reference mode: "none", "config", or "auto"
    pub reference: String,
    /// Override versions for packages
    pub versions: HashMap<String, String>,
}

/// Path repository - loads packages from local filesystem
pub struct PathRepository {
    /// Repository name
    name: String,
    /// Base URL/path pattern (may contain glob patterns)
    url: String,
    /// Resolved absolute path
    resolved_path: PathBuf,
    /// Repository options
    options: PathRepositoryOptions,
    /// Cached packages (with interior mutability for lazy loading)
    packages: RwLock<Option<Vec<Arc<Package>>>>,
}

impl PathRepository {
    /// Create a new path repository
    ///
    /// # Arguments
    /// * `url` - Path to the package(s), can contain glob patterns like `packages/*`
    /// * `options` - Repository options
    pub fn new(url: impl Into<String>, options: PathRepositoryOptions) -> Self {
        let url = url.into();

        // Expand ~ to home directory
        let expanded = shellexpand::tilde(&url).to_string();

        // Resolve to absolute path
        let resolved_path = if Path::new(&expanded).is_absolute() {
            PathBuf::from(&expanded)
        } else {
            std::env::current_dir()
                .unwrap_or_default()
                .join(&expanded)
        };

        Self {
            name: format!("path repo ({})", url),
            url,
            resolved_path,
            options,
            packages: RwLock::new(None),
        }
    }

    /// Create a path repository with default options
    pub fn from_path(url: impl Into<String>) -> Self {
        Self::new(url, PathRepositoryOptions {
            reference: "auto".to_string(),
            ..Default::default()
        })
    }

    /// Get all matching paths (handles glob patterns)
    fn get_url_matches(&self) -> Vec<PathBuf> {
        let path_str = self.resolved_path.to_string_lossy();

        // Check if path contains glob patterns
        if path_str.contains('*') || path_str.contains('?') || path_str.contains('[') {
            match glob(&path_str) {
                Ok(paths) => paths
                    .filter_map(|p| p.ok())
                    .filter(|p| p.is_dir())
                    .collect(),
                Err(_) => Vec::new(),
            }
        } else if self.resolved_path.is_dir() {
            vec![self.resolved_path.clone()]
        } else {
            Vec::new()
        }
    }

    /// Load packages from all matching paths
    async fn ensure_loaded(&self) -> Vec<Arc<Package>> {
        // Check if already loaded
        {
            let guard = self.packages.read().await;
            if let Some(ref pkgs) = *guard {
                return pkgs.clone();
            }
        }

        // Load packages
        let matches = self.get_url_matches();
        let mut packages = Vec::new();

        for path in matches {
            if let Some(pkg) = self.load_package_from_path(&path) {
                packages.push(Arc::new(pkg));
            }
        }

        // Store in cache
        {
            let mut guard = self.packages.write().await;
            *guard = Some(packages.clone());
        }

        packages
    }

    /// Load a single package from a directory
    fn load_package_from_path(&self, path: &Path) -> Option<Package> {
        let composer_json = path.join("composer.json");

        if !composer_json.exists() {
            return None;
        }

        // Read and parse composer.json
        let content = std::fs::read_to_string(&composer_json).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;

        let name = json.get("name")?.as_str()?;

        let version = self.determine_version(&json, path, name);

        let mut pkg = Package::new(name, &version);

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

        let reference = self.compute_reference(&content, path);
        let mut dist = Dist::new("path", path.to_string_lossy().as_ref());
        if let Some(ref r) = reference {
            dist = dist.with_reference(r);
        }

        let mut transport_options = std::collections::HashMap::new();
        if let Some(symlink) = self.options.symlink {
            transport_options.insert("symlink".to_string(), serde_json::Value::Bool(symlink));
        }
        if self.options.relative {
            transport_options.insert("relative".to_string(), serde_json::Value::Bool(true));
        }
        if !transport_options.is_empty() {
            dist = dist.with_transport_options(transport_options);
        }

        pkg.dist = Some(dist);

        if path.join(".git").exists() {
            if let Some(git_ref) = get_git_reference(path) {
                let git_url = get_git_url(path).unwrap_or_else(|| path.to_string_lossy().to_string());
                pkg.source = Some(Source::new("git", &git_url, &git_ref));
            }
        }

        // Replace self.version constraints with actual version
        pkg.replace_self_version();

        Some(pkg)
    }

    /// Determine the version for a package
    fn determine_version(&self, json: &serde_json::Value, path: &Path, name: &str) -> String {
        // 1. Check for version override in options
        if let Some(version) = self.options.versions.get(name) {
            return version.clone();
        }

        // 2. Check for explicit version in composer.json
        if let Some(version) = json.get("version").and_then(|v| v.as_str()) {
            return version.to_string();
        }

        // 3. Try to guess version from VCS
        if let Some(version) = guess_version_from_vcs(path) {
            return version;
        }

        // 4. Default to dev-main
        "dev-main".to_string()
    }

    /// Compute the reference for the dist
    fn compute_reference(&self, content: &str, path: &Path) -> Option<String> {
        match self.options.reference.as_str() {
            "none" => None,
            "config" => {
                // Hash of composer.json content + options
                let mut hasher = Sha1::new();
                hasher.update(content.as_bytes());
                hasher.update(format!("{:?}", self.options).as_bytes());
                Some(format!("{:x}", hasher.finalize()))
            }
            "auto" | _ => {
                // Try git commit hash first
                if let Some(git_ref) = get_git_reference(path) {
                    return Some(git_ref);
                }
                // Fall back to config hash
                let mut hasher = Sha1::new();
                hasher.update(content.as_bytes());
                hasher.update(format!("{:?}", self.options).as_bytes());
                Some(format!("{:x}", hasher.finalize()))
            }
        }
    }

    /// Get the URL pattern
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Get the options
    pub fn options(&self) -> &PathRepositoryOptions {
        &self.options
    }
}

#[async_trait]
impl Repository for PathRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn has_package(&self, name: &str) -> bool {
        !self.find_packages(name).await.is_empty()
    }

    async fn find_packages(&self, name: &str) -> Vec<Arc<Package>> {
        let packages = self.ensure_loaded().await;

        packages
            .into_iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
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
        // For path repositories, we typically have only one version
        // Return all matching packages
        self.find_packages(name).await
    }

    async fn get_packages(&self) -> Vec<Arc<Package>> {
        self.ensure_loaded().await
    }

    async fn search(&self, query: &str, _mode: SearchMode) -> Vec<SearchResult> {
        let packages = self.ensure_loaded().await;

        packages
            .iter()
            .filter(|p| {
                p.name.contains(query) ||
                p.description.as_ref().map(|d| d.contains(query)).unwrap_or(false)
            })
            .map(|p| SearchResult {
                name: p.name.clone(),
                description: p.description.clone(),
                url: None,
                abandoned: None,
            })
            .collect()
    }

    async fn get_providers(&self, _package_name: &str) -> Vec<ProviderInfo> {
        Vec::new()
    }
}

/// Get the current git commit hash
fn get_git_reference(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Get the git remote URL
fn get_git_url(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(path)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Guess version from VCS (git)
fn guess_version_from_vcs(path: &Path) -> Option<String> {
    // Check if it's a git repository
    if !path.join(".git").exists() {
        return None;
    }

    // Try to get the current branch
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Check for tags
    let tag_output = Command::new("git")
        .args(["describe", "--tags", "--exact-match", "HEAD"])
        .current_dir(path)
        .output()
        .ok();

    if let Some(tag_output) = tag_output {
        if tag_output.status.success() {
            let tag = String::from_utf8_lossy(&tag_output.stdout).trim().to_string();
            // Strip 'v' prefix if present
            let version = tag.strip_prefix('v').unwrap_or(&tag);
            return Some(version.to_string());
        }
    }

    // Return branch as dev version
    Some(format!("dev-{}", branch))
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
    use tempfile::TempDir;

    fn create_test_package(dir: &Path, name: &str, version: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();

        let mut json = serde_json::json!({
            "name": name,
            "description": "Test package"
        });

        if let Some(v) = version {
            json["version"] = serde_json::Value::String(v.to_string());
        }

        std::fs::write(
            dir.join("composer.json"),
            serde_json::to_string_pretty(&json).unwrap(),
        ).unwrap();
    }

    #[tokio::test]
    async fn test_path_repository_single_package() {
        let temp = TempDir::new().unwrap();
        let pkg_dir = temp.path().join("my-package");
        create_test_package(&pkg_dir, "vendor/my-package", Some("1.0.0"));

        let repo = PathRepository::from_path(pkg_dir.to_string_lossy().to_string());

        let packages = repo.get_packages().await;
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "vendor/my-package");
        assert_eq!(packages[0].version, "1.0.0");
    }

    #[tokio::test]
    async fn test_path_repository_glob_pattern() {
        let temp = TempDir::new().unwrap();

        // Create multiple packages
        create_test_package(&temp.path().join("packages/pkg-a"), "vendor/pkg-a", Some("1.0.0"));
        create_test_package(&temp.path().join("packages/pkg-b"), "vendor/pkg-b", Some("2.0.0"));

        let pattern = temp.path().join("packages/*").to_string_lossy().to_string();
        let repo = PathRepository::from_path(pattern);

        let packages = repo.get_packages().await;
        assert_eq!(packages.len(), 2);
    }

    #[tokio::test]
    async fn test_path_repository_version_override() {
        let temp = TempDir::new().unwrap();
        let pkg_dir = temp.path().join("my-package");
        create_test_package(&pkg_dir, "vendor/my-package", Some("1.0.0"));

        let mut versions = HashMap::new();
        versions.insert("vendor/my-package".to_string(), "2.0.0".to_string());

        let options = PathRepositoryOptions {
            versions,
            ..Default::default()
        };

        let repo = PathRepository::new(pkg_dir.to_string_lossy().to_string(), options);

        let packages = repo.get_packages().await;
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].version, "2.0.0");
    }

    #[tokio::test]
    async fn test_path_repository_find_package() {
        let temp = TempDir::new().unwrap();
        let pkg_dir = temp.path().join("my-package");
        create_test_package(&pkg_dir, "vendor/my-package", Some("1.0.0"));

        let repo = PathRepository::from_path(pkg_dir.to_string_lossy().to_string());

        let found = repo.find_packages("vendor/my-package").await;
        assert_eq!(found.len(), 1);

        let not_found = repo.find_packages("vendor/other").await;
        assert!(not_found.is_empty());
    }
}
