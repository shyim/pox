//! Package repository - inline package definitions in composer.json.

use std::sync::Arc;

use async_trait::async_trait;

use super::traits::{Repository, SearchMode, SearchResult, ProviderInfo};
use crate::package::{Package, Dist, Source, Autoload, AutoloadPath};

/// Package repository - provides packages from inline definitions
///
/// This repository type allows defining packages directly in composer.json:
///
/// ```json
/// {
///     "repositories": [
///         {
///             "type": "package",
///             "package": {
///                 "name": "vendor/package",
///                 "version": "1.0.0",
///                 "dist": {
///                     "url": "https://example.com/package.zip",
///                     "type": "zip"
///                 }
///             }
///         }
///     ]
/// }
/// ```
///
/// Multiple versions can be defined using an array:
///
/// ```json
/// {
///     "repositories": [
///         {
///             "type": "package",
///             "package": [
///                 { "name": "vendor/package", "version": "1.0.0", ... },
///                 { "name": "vendor/package", "version": "2.0.0", ... }
///             ]
///         }
///     ]
/// }
/// ```
#[derive(Debug)]
pub struct PackageRepository {
    /// Repository name
    name: String,
    /// Loaded packages
    packages: Vec<Arc<Package>>,
}

impl PackageRepository {
    /// Create a new package repository from inline package definition(s)
    ///
    /// # Arguments
    /// * `package_config` - Either a single package object or an array of package objects
    pub fn new(package_config: &serde_json::Value) -> Result<Self, String> {
        let mut packages = Vec::new();

        // Handle both single package and array of packages
        let package_array = if package_config.is_array() {
            package_config.as_array().unwrap().clone()
        } else if package_config.is_object() {
            vec![package_config.clone()]
        } else {
            return Err("Package config must be an object or array".to_string());
        };

        for (index, pkg_json) in package_array.iter().enumerate() {
            let pkg = Self::load_package(pkg_json)
                .map_err(|e| format!("Invalid package at index {}: {}", index, e))?;
            packages.push(Arc::new(pkg));
        }

        let name = if packages.len() == 1 {
            format!("package {}", packages[0].name)
        } else {
            format!("package repo ({} packages)", packages.len())
        };

        Ok(Self { name, packages })
    }

    /// Load a single package from JSON
    fn load_package(json: &serde_json::Value) -> Result<Package, String> {
        // Required fields
        let name = json.get("name")
            .and_then(|v| v.as_str())
            .ok_or("Package must have a 'name' field")?;

        let version = json.get("version")
            .and_then(|v| v.as_str())
            .ok_or("Package must have a 'version' field")?;

        // Must have either dist or source
        let has_dist = json.get("dist").is_some();
        let has_source = json.get("source").is_some();

        if !has_dist && !has_source {
            return Err("Package must have either 'dist' or 'source'".to_string());
        }

        let mut pkg = Package::new(name, version);

        // Parse dist
        if let Some(dist_json) = json.get("dist") {
            pkg.dist = Some(Self::parse_dist(dist_json)?);
        }

        // Parse source
        if let Some(source_json) = json.get("source") {
            pkg.source = Some(Self::parse_source(source_json)?);
        }

        // Optional fields
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

        // Extra metadata
        if let Some(homepage) = json.get("homepage").and_then(|v| v.as_str()) {
            pkg.homepage = Some(homepage.to_string());
        }

        if let Some(keywords) = json.get("keywords").and_then(|v| v.as_array()) {
            pkg.keywords = keywords
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }

        // Replace self.version constraints with actual version
        pkg.replace_self_version();

        Ok(pkg)
    }

    /// Parse dist configuration
    fn parse_dist(json: &serde_json::Value) -> Result<Dist, String> {
        let dist_type = json.get("type")
            .and_then(|v| v.as_str())
            .ok_or("dist must have a 'type' field")?;

        let url = json.get("url")
            .and_then(|v| v.as_str())
            .ok_or("dist must have a 'url' field")?;

        let mut dist = Dist::new(dist_type, url);

        if let Some(reference) = json.get("reference").and_then(|v| v.as_str()) {
            dist = dist.with_reference(reference);
        }

        if let Some(shasum) = json.get("shasum").and_then(|v| v.as_str()) {
            dist = dist.with_shasum(shasum);
        }

        Ok(dist)
    }

    /// Parse source configuration
    fn parse_source(json: &serde_json::Value) -> Result<Source, String> {
        let source_type = json.get("type")
            .and_then(|v| v.as_str())
            .ok_or("source must have a 'type' field")?;

        let url = json.get("url")
            .and_then(|v| v.as_str())
            .ok_or("source must have a 'url' field")?;

        let reference = json.get("reference")
            .and_then(|v| v.as_str())
            .ok_or("source must have a 'reference' field")?;

        Ok(Source::new(source_type, url, reference))
    }
}

#[async_trait]
impl Repository for PackageRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn has_package(&self, name: &str) -> bool {
        self.packages.iter().any(|p| p.name.eq_ignore_ascii_case(name))
    }

    async fn find_packages(&self, name: &str) -> Vec<Arc<Package>> {
        self.packages
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .cloned()
            .collect()
    }

    async fn find_package(&self, name: &str, version: &str) -> Option<Arc<Package>> {
        self.packages
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name) && p.version == version)
            .cloned()
    }

    async fn find_packages_with_constraint(
        &self,
        name: &str,
        _constraint: &str,
    ) -> Vec<Arc<Package>> {
        // For inline packages, return all versions matching the name
        // The solver will filter by constraint
        self.find_packages(name).await
    }

    async fn get_packages(&self) -> Vec<Arc<Package>> {
        self.packages.clone()
    }

    async fn search(&self, query: &str, _mode: SearchMode) -> Vec<SearchResult> {
        self.packages
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

    #[tokio::test]
    async fn test_single_package() {
        let config = serde_json::json!({
            "name": "vendor/package",
            "version": "1.0.0",
            "dist": {
                "url": "https://example.com/package.zip",
                "type": "zip"
            }
        });

        let repo = PackageRepository::new(&config).unwrap();
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "vendor/package");
        assert_eq!(packages[0].version, "1.0.0");
        assert!(packages[0].dist.is_some());
    }

    #[tokio::test]
    async fn test_multiple_packages() {
        let config = serde_json::json!([
            {
                "name": "vendor/package",
                "version": "1.0.0",
                "dist": {
                    "url": "https://example.com/package-1.0.0.zip",
                    "type": "zip"
                }
            },
            {
                "name": "vendor/package",
                "version": "2.0.0",
                "dist": {
                    "url": "https://example.com/package-2.0.0.zip",
                    "type": "zip"
                }
            }
        ]);

        let repo = PackageRepository::new(&config).unwrap();
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 2);
    }

    #[tokio::test]
    async fn test_package_with_source() {
        let config = serde_json::json!({
            "name": "vendor/package",
            "version": "1.0.0",
            "source": {
                "url": "https://github.com/vendor/package.git",
                "type": "git",
                "reference": "abc123"
            }
        });

        let repo = PackageRepository::new(&config).unwrap();
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 1);
        assert!(packages[0].source.is_some());
        assert_eq!(packages[0].source.as_ref().unwrap().reference, "abc123");
    }

    #[tokio::test]
    async fn test_package_with_both_dist_and_source() {
        let config = serde_json::json!({
            "name": "vendor/package",
            "version": "1.0.0",
            "dist": {
                "url": "https://example.com/package.zip",
                "type": "zip"
            },
            "source": {
                "url": "https://github.com/vendor/package.git",
                "type": "git",
                "reference": "abc123"
            }
        });

        let repo = PackageRepository::new(&config).unwrap();
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 1);
        assert!(packages[0].dist.is_some());
        assert!(packages[0].source.is_some());
    }

    #[tokio::test]
    async fn test_package_with_metadata() {
        let config = serde_json::json!({
            "name": "vendor/package",
            "version": "1.0.0",
            "description": "A test package",
            "type": "library",
            "license": "MIT",
            "require": {
                "php": ">=8.0"
            },
            "autoload": {
                "psr-4": {
                    "Vendor\\Package\\": "src/"
                }
            },
            "dist": {
                "url": "https://example.com/package.zip",
                "type": "zip"
            }
        });

        let repo = PackageRepository::new(&config).unwrap();
        let packages = repo.get_packages().await;

        assert_eq!(packages[0].description, Some("A test package".to_string()));
        assert_eq!(packages[0].package_type, "library");
        assert_eq!(packages[0].license, vec!["MIT".to_string()]);
        assert!(packages[0].require.contains_key("php"));
        assert!(packages[0].autoload.is_some());
    }

    #[tokio::test]
    async fn test_find_package() {
        let config = serde_json::json!([
            {
                "name": "vendor/package",
                "version": "1.0.0",
                "dist": { "url": "https://example.com/1.zip", "type": "zip" }
            },
            {
                "name": "vendor/package",
                "version": "2.0.0",
                "dist": { "url": "https://example.com/2.zip", "type": "zip" }
            }
        ]);

        let repo = PackageRepository::new(&config).unwrap();

        let found = repo.find_package("vendor/package", "1.0.0").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().version, "1.0.0");

        let found = repo.find_package("vendor/package", "2.0.0").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().version, "2.0.0");

        let not_found = repo.find_package("vendor/package", "3.0.0").await;
        assert!(not_found.is_none());
    }

    #[test]
    fn test_missing_name() {
        let config = serde_json::json!({
            "version": "1.0.0",
            "dist": { "url": "https://example.com/package.zip", "type": "zip" }
        });

        let result = PackageRepository::new(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("name"));
    }

    #[test]
    fn test_missing_version() {
        let config = serde_json::json!({
            "name": "vendor/package",
            "dist": { "url": "https://example.com/package.zip", "type": "zip" }
        });

        let result = PackageRepository::new(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("version"));
    }

    #[test]
    fn test_missing_dist_and_source() {
        let config = serde_json::json!({
            "name": "vendor/package",
            "version": "1.0.0"
        });

        let result = PackageRepository::new(&config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("dist") || err.contains("source"));
    }

    #[test]
    fn test_dist_missing_type() {
        let config = serde_json::json!({
            "name": "vendor/package",
            "version": "1.0.0",
            "dist": {
                "url": "https://example.com/package.zip"
            }
        });

        let result = PackageRepository::new(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("type"));
    }

    #[test]
    fn test_source_missing_reference() {
        let config = serde_json::json!({
            "name": "vendor/package",
            "version": "1.0.0",
            "source": {
                "url": "https://github.com/vendor/package.git",
                "type": "git"
            }
        });

        let result = PackageRepository::new(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("reference"));
    }

    #[tokio::test]
    async fn test_search() {
        let config = serde_json::json!([
            {
                "name": "vendor/foo-package",
                "version": "1.0.0",
                "description": "A foo package",
                "dist": { "url": "https://example.com/foo.zip", "type": "zip" }
            },
            {
                "name": "vendor/bar-package",
                "version": "1.0.0",
                "description": "A bar package",
                "dist": { "url": "https://example.com/bar.zip", "type": "zip" }
            }
        ]);

        let repo = PackageRepository::new(&config).unwrap();

        let results = repo.search("foo", SearchMode::Name).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "vendor/foo-package");
    }
}
