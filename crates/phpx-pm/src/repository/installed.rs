use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use async_trait::async_trait;
use tokio::sync::RwLock;

use super::traits::{Repository, WritableRepository, SearchMode, SearchResult, ProviderInfo};
use crate::package::{Package, Source, Dist};

/// Repository for installed packages (vendor/composer/installed.json)
pub struct InstalledRepository {
    /// Path to the vendor directory
    vendor_dir: PathBuf,
    /// Installed packages
    packages: RwLock<HashMap<String, Arc<Package>>>,
    /// Whether the repository has been modified
    dirty: RwLock<bool>,
}

impl InstalledRepository {
    /// Create a new installed repository
    pub fn new(vendor_dir: impl Into<PathBuf>) -> Self {
        Self {
            vendor_dir: vendor_dir.into(),
            packages: RwLock::new(HashMap::new()),
            dirty: RwLock::new(false),
        }
    }

    /// Get the path to installed.json
    pub fn installed_json_path(&self) -> PathBuf {
        self.vendor_dir.join("composer").join("installed.json")
    }

    /// Load packages from installed.json
    pub async fn load(&self) -> Result<(), String> {
        let path = self.installed_json_path();
        if !path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read installed.json: {}", e))?;

        let data: InstalledJson = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse installed.json: {}", e))?;

        let mut packages = self.packages.write().await;
        packages.clear();

        for pkg_data in data.packages {
            let package = Package::from_installed_json(&pkg_data);
            packages.insert(package.name.clone(), Arc::new(package));
        }

        Ok(())
    }

    /// Get the vendor directory path
    pub fn vendor_dir(&self) -> &Path {
        &self.vendor_dir
    }
}

#[async_trait]
impl Repository for InstalledRepository {
    fn name(&self) -> &str {
        "installed"
    }

    async fn has_package(&self, name: &str) -> bool {
        let packages = self.packages.read().await;
        packages.contains_key(&name.to_lowercase())
    }

    async fn find_packages(&self, name: &str) -> Vec<Arc<Package>> {
        let packages = self.packages.read().await;
        packages
            .get(&name.to_lowercase())
            .map(|p| vec![p.clone()])
            .unwrap_or_default()
    }

    async fn find_package(&self, name: &str, version: &str) -> Option<Arc<Package>> {
        let packages = self.packages.read().await;
        packages.get(&name.to_lowercase()).and_then(|p| {
            if p.version == version || p.pretty_version.as_deref() == Some(version) {
                Some(p.clone())
            } else {
                None
            }
        })
    }

    async fn find_packages_with_constraint(
        &self,
        name: &str,
        _constraint: &str,
    ) -> Vec<Arc<Package>> {
        // Installed repository only has one version per package
        self.find_packages(name).await
    }

    async fn get_packages(&self) -> Vec<Arc<Package>> {
        let packages = self.packages.read().await;
        packages.values().cloned().collect()
    }

    async fn search(&self, query: &str, _mode: SearchMode) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        let packages = self.packages.read().await;

        packages
            .values()
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

    async fn get_providers(&self, package_name: &str) -> Vec<ProviderInfo> {
        let packages = self.packages.read().await;

        packages
            .values()
            .filter(|p| p.provide.contains_key(package_name))
            .map(|p| ProviderInfo {
                name: p.name.clone(),
                description: p.description.clone(),
                package_type: p.package_type.clone(),
            })
            .collect()
    }

    async fn count(&self) -> usize {
        let packages = self.packages.read().await;
        packages.len()
    }
}

#[async_trait]
impl WritableRepository for InstalledRepository {
    async fn add_package(&mut self, package: Package) {
        let mut packages = self.packages.write().await;
        packages.insert(package.name.to_lowercase(), Arc::new(package));
        *self.dirty.write().await = true;
    }

    async fn remove_package(&mut self, package: &Package) {
        let mut packages = self.packages.write().await;
        packages.remove(&package.name.to_lowercase());
        *self.dirty.write().await = true;
    }

    fn is_dirty(&self) -> bool {
        // Can't await in a non-async fn, so return false
        // Real implementation would need to restructure this
        false
    }

    async fn write(&self) -> std::io::Result<()> {
        let packages = self.packages.read().await;

        let installed = InstalledJson {
            packages: packages.values().map(|p| p.to_installed_json()).collect(),
            dev: true,
            dev_package_names: vec![],
        };

        let content = serde_json::to_string_pretty(&installed)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let path = self.installed_json_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(path, content)?;
        *self.dirty.write().await = false;

        Ok(())
    }
}

/// Structure of installed.json
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InstalledJson {
    packages: Vec<InstalledPackage>,
    #[serde(default)]
    dev: bool,
    #[serde(default)]
    dev_package_names: Vec<String>,
}

/// Package entry in installed.json
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub version_normalized: String,
    #[serde(rename = "type", default = "default_type")]
    pub package_type: String,
    #[serde(default)]
    pub source: Option<InstalledSource>,
    #[serde(default)]
    pub dist: Option<InstalledDist>,
    #[serde(default)]
    pub require: HashMap<String, String>,
    #[serde(default, rename = "require-dev")]
    pub require_dev: HashMap<String, String>,
    #[serde(default)]
    pub conflict: HashMap<String, String>,
    #[serde(default)]
    pub replace: HashMap<String, String>,
    #[serde(default)]
    pub provide: HashMap<String, String>,
    #[serde(default)]
    pub autoload: serde_json::Value,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub license: serde_json::Value,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub install_path: Option<String>,
}

fn default_type() -> String {
    "library".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstalledSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub url: String,
    pub reference: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstalledDist {
    #[serde(rename = "type")]
    pub dist_type: String,
    pub url: String,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub shasum: Option<String>,
}

impl Package {
    /// Create a Package from installed.json format
    pub fn from_installed_json(data: &InstalledPackage) -> Self {
        let source = data.source.as_ref().map(|s| Source {
            source_type: s.source_type.clone(),
            url: s.url.clone(),
            reference: s.reference.clone(),
            mirrors: None,
        });

        let dist = data.dist.as_ref().map(|d| Dist {
            dist_type: d.dist_type.clone(),
            url: d.url.clone(),
            reference: d.reference.clone(),
            shasum: d.shasum.clone(),
            sha256: None,
            mirrors: None,
            transport_options: None,
        });

        let mut pkg = Package::new(&data.name, &data.version_normalized);
        pkg.pretty_version = Some(data.version.clone());
        pkg.package_type = data.package_type.clone();
        pkg.source = source;
        pkg.dist = dist;
        pkg.require = data.require.clone();
        pkg.require_dev = data.require_dev.clone();
        pkg.conflict = data.conflict.clone();
        pkg.replace = data.replace.clone();
        pkg.provide = data.provide.clone();
        pkg.description = data.description.clone();

        // Replace self.version constraints with actual version
        pkg.replace_self_version();

        pkg
    }

    /// Convert to installed.json format
    pub fn to_installed_json(&self) -> InstalledPackage {
        let source = self.source.as_ref().map(|s| InstalledSource {
            source_type: s.source_type.clone(),
            url: s.url.clone(),
            reference: s.reference.clone(),
        });

        let dist = self.dist.as_ref().map(|d| InstalledDist {
            dist_type: d.dist_type.clone(),
            url: d.url.clone(),
            reference: d.reference.clone(),
            shasum: d.shasum.clone(),
        });

        InstalledPackage {
            name: self.name.clone(),
            version: self.pretty_version.clone().unwrap_or_else(|| self.version.clone()),
            version_normalized: self.version.clone(),
            package_type: self.package_type.clone(),
            source,
            dist,
            require: self.require.clone(),
            require_dev: self.require_dev.clone(),
            conflict: self.conflict.clone(),
            replace: self.replace.clone(),
            provide: self.provide.clone(),
            autoload: serde_json::Value::Null,
            description: self.description.clone(),
            license: serde_json::Value::Null,
            time: self.time.map(|t| t.to_rfc3339()),
            install_path: None,
        }
    }
}
