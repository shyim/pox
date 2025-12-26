use std::sync::Arc;
use std::collections::HashMap;
use async_trait::async_trait;

use super::traits::{Repository, SearchMode, SearchResult, ProviderInfo};
use crate::package::{Package, Stability};
use phpx_semver::{Constraint, Operator, VersionParser};

/// Platform repository - provides PHP version and extensions
pub struct PlatformRepository {
    /// Cached platform packages
    packages: Vec<Arc<Package>>,
    /// Platform overrides from config
    overrides: HashMap<String, String>,
    /// Disabled packages
    disabled: Vec<String>,
}

impl PlatformRepository {
    /// Create a new platform repository with auto-detection
    pub fn new() -> Self {
        Self {
            packages: Vec::new(),
            overrides: HashMap::new(),
            disabled: Vec::new(),
        }
    }

    /// Create with platform overrides
    pub fn with_overrides(overrides: HashMap<String, String>) -> Self {
        let disabled: Vec<String> = overrides
            .iter()
            .filter(|(_, v)| *v == "false" || v.is_empty())
            .map(|(k, _)| k.clone())
            .collect();

        Self {
            packages: Vec::new(),
            overrides,
            disabled,
        }
    }

    /// Detect platform packages from the current PHP installation
    pub fn detect(&mut self) {
        // In a real implementation, this would:
        // 1. Run `php -v` to get PHP version
        // 2. Run `php -m` to get loaded extensions
        // 3. Query extension versions where available

        // For now, add placeholder packages
        self.packages.clear();

        // Add PHP package
        if !self.disabled.contains(&"php".to_string()) {
            let php_version = self.overrides.get("php").cloned().unwrap_or_else(|| "8.3.0".to_string());
            let mut pkg = Package::new("php", normalize_version(&php_version));
            pkg.pretty_version = Some(php_version);
            pkg.package_type = "platform".to_string();
            pkg.stability = Some(Stability::Stable);
            pkg.description = Some("The PHP interpreter".to_string());
            self.packages.push(Arc::new(pkg));
        }

        // Add common extensions
        let common_extensions = [
            ("ext-json", "JSON extension"),
            ("ext-mbstring", "Multibyte string extension"),
            ("ext-openssl", "OpenSSL extension"),
            ("ext-pdo", "PHP Data Objects"),
            ("ext-curl", "cURL extension"),
            ("ext-dom", "DOM extension"),
            ("ext-xml", "XML extension"),
            ("ext-ctype", "Ctype extension"),
            ("ext-tokenizer", "Tokenizer extension"),
        ];

        for (ext, desc) in common_extensions {
            if !self.disabled.contains(&ext.to_string()) {
                let version = self.overrides.get(ext).cloned().unwrap_or_else(|| "8.3.0".to_string());
                let mut pkg = Package::new(ext, normalize_version(&version));
                pkg.pretty_version = Some(version);
                pkg.package_type = "platform".to_string();
                pkg.stability = Some(Stability::Stable);
                pkg.description = Some(desc.to_string());
                self.packages.push(Arc::new(pkg));
            }
        }

        // Add composer packages
        let mut composer_pkg = Package::new("composer", "2.99.99.0");
        composer_pkg.pretty_version = Some("2.99.99".to_string());
        composer_pkg.package_type = "platform".to_string();
        composer_pkg.stability = Some(Stability::Stable);
        composer_pkg.description = Some("Composer package manager".to_string());
        self.packages.push(Arc::new(composer_pkg));

        let mut runtime_pkg = Package::new("composer-runtime-api", "2.2.2.0");
        runtime_pkg.pretty_version = Some("2.2.2".to_string());
        runtime_pkg.package_type = "platform".to_string();
        runtime_pkg.stability = Some(Stability::Stable);
        runtime_pkg.description = Some("Composer runtime API".to_string());
        self.packages.push(Arc::new(runtime_pkg));

        let mut plugin_pkg = Package::new("composer-plugin-api", "2.6.0.0");
        plugin_pkg.pretty_version = Some("2.6.0".to_string());
        plugin_pkg.package_type = "platform".to_string();
        plugin_pkg.stability = Some(Stability::Stable);
        plugin_pkg.description = Some("Composer plugin API".to_string());
        self.packages.push(Arc::new(plugin_pkg));
    }
}

/// Normalize a version string to Composer format
fn normalize_version(version: &str) -> String {
    // Simple normalization - just ensure 4 parts
    let parts: Vec<&str> = version.split('.').collect();
    match parts.len() {
        1 => format!("{}.0.0.0", parts[0]),
        2 => format!("{}.{}.0.0", parts[0], parts[1]),
        3 => format!("{}.{}.{}.0", parts[0], parts[1], parts[2]),
        _ => version.to_string(),
    }
}

impl Default for PlatformRepository {
    fn default() -> Self {
        let mut repo = Self::new();
        repo.detect();
        repo
    }
}

#[async_trait]
impl Repository for PlatformRepository {
    fn name(&self) -> &str {
        "platform"
    }

    async fn has_package(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();
        self.packages.iter().any(|p| p.name.to_lowercase() == name_lower)
    }

    async fn find_packages(&self, name: &str) -> Vec<Arc<Package>> {
        let name_lower = name.to_lowercase();
        self.packages
            .iter()
            .filter(|p| p.name.to_lowercase() == name_lower)
            .cloned()
            .collect()
    }

    async fn find_package(&self, name: &str, version: &str) -> Option<Arc<Package>> {
        let name_lower = name.to_lowercase();
        self.packages
            .iter()
            .find(|p| p.name.to_lowercase() == name_lower && p.version == version)
            .cloned()
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
        self.packages.clone()
    }

    async fn search(&self, query: &str, _mode: SearchMode) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        self.packages
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&query_lower))
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

    async fn count(&self) -> usize {
        self.packages.len()
    }
}
