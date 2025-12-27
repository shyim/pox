//! Installation manager - orchestrates package installation.

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};

use crate::downloader::{DownloadConfig, DownloadManager};
use crate::http::HttpClient;
use crate::package::Package;
use crate::solver::{Operation, Transaction};
use crate::Result;

use super::binary::BinaryInstaller;
use super::library::LibraryInstaller;
use super::metapackage::MetapackageInstaller;

/// Installation configuration
#[derive(Debug, Clone)]
pub struct InstallConfig {
    /// Vendor directory
    pub vendor_dir: PathBuf,
    /// Bin directory
    pub bin_dir: PathBuf,
    /// Cache directory
    pub cache_dir: PathBuf,
    /// Prefer source over dist
    pub prefer_source: bool,
    /// Prefer dist over source
    pub prefer_dist: bool,
    /// Run in dry-run mode (no actual changes)
    pub dry_run: bool,
    /// Skip dev dependencies
    pub no_dev: bool,
    /// Prefer lowest versions (useful for testing compatibility)
    pub prefer_lowest: bool,
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            vendor_dir: PathBuf::from("vendor"),
            bin_dir: PathBuf::from("vendor/bin"),
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from(".composer"))
                .join("cache"),
            prefer_source: false,
            prefer_dist: true,
            dry_run: false,
            no_dev: false,
            prefer_lowest: false,
        }
    }
}

const MAX_CONCURRENT_INSTALLS: usize = 10;

/// Installation manager
pub struct InstallationManager {
    library_installer: Arc<LibraryInstaller>,
    binary_installer: Arc<BinaryInstaller>,
    metapackage_installer: MetapackageInstaller,
    config: InstallConfig,
}

/// Result of an installation operation
#[derive(Debug)]
pub struct InstallResult {
    /// Packages that were installed
    pub installed: Vec<Package>,
    /// Packages that were updated (from, to)
    pub updated: Vec<(Package, Package)>,
    /// Packages that were removed
    pub removed: Vec<Package>,
    /// Binaries that were linked
    pub binaries: Vec<PathBuf>,
}

impl InstallationManager {
    /// Create a new installation manager
    pub fn new(http_client: Arc<HttpClient>, config: InstallConfig) -> Self {
        let download_config = DownloadConfig {
            vendor_dir: config.vendor_dir.clone(),
            cache_dir: config.cache_dir.clone(),
            prefer_source: config.prefer_source,
            prefer_dist: config.prefer_dist,
        };

        let download_manager = Arc::new(DownloadManager::new(http_client, download_config));

        let library_installer = Arc::new(LibraryInstaller::new(
            download_manager,
            config.vendor_dir.clone(),
        ));

        let binary_installer = Arc::new(BinaryInstaller::new(
            config.bin_dir.clone(),
            config.vendor_dir.clone(),
        ));

        let metapackage_installer = MetapackageInstaller::new();

        Self {
            library_installer,
            binary_installer,
            metapackage_installer,
            config,
        }
    }

    /// Execute a transaction (install/update/remove packages)
    pub async fn execute(&self, transaction: &Transaction) -> Result<InstallResult> {
        let mut result = InstallResult {
            installed: Vec::new(),
            updated: Vec::new(),
            removed: Vec::new(),
            binaries: Vec::new(),
        };

        if self.config.dry_run {
            // In dry-run mode, just collect what would be done
            for op in &transaction.operations {
                match op {
                    Operation::Install(pkg) => {
                        result.installed.push(pkg.as_ref().clone());
                    }
                    Operation::Update { from, to } => {
                        result.updated.push((from.as_ref().clone(), to.as_ref().clone()));
                    }
                    Operation::Uninstall(pkg) => {
                        result.removed.push(pkg.as_ref().clone());
                    }
                    Operation::MarkUnneeded(_) => {}
                    // Alias operations don't need any file system changes
                    Operation::MarkAliasInstalled(_) | Operation::MarkAliasUninstalled(_) => {}
                }
            }
            return Ok(result);
        }

        // Create vendor directory
        tokio::fs::create_dir_all(&self.config.vendor_dir).await?;

        // Separate operations into phases for parallel execution:
        // 1. Uninstalls must happen first (sequential - usually few)
        // 2. Updates can be parallelized (remove old, install new)
        // 3. Installs can be parallelized

        let mut uninstalls = Vec::new();
        let mut updates = Vec::new();
        let mut installs = Vec::new();

        for op in &transaction.operations {
            match op {
                Operation::Uninstall(pkg) => {
                    if !pkg.is_platform_package() {
                        uninstalls.push(pkg.clone());
                    }
                }
                Operation::Update { from, to } => {
                    if !to.is_platform_package() {
                        updates.push((from.clone(), to.clone()));
                    }
                }
                Operation::Install(pkg) => {
                    if !pkg.is_platform_package() {
                        installs.push(pkg.clone());
                    }
                }
                Operation::MarkUnneeded(_)
                | Operation::MarkAliasInstalled(_)
                | Operation::MarkAliasUninstalled(_) => {}
            }
        }

        // Phase 1: Process uninstalls (sequential, usually few)
        for pkg in &uninstalls {
            if pkg.is_metapackage() {
                self.metapackage_installer.uninstall(pkg).await?;
            } else {
                self.binary_installer.uninstall(pkg).await?;
                self.uninstall_package(pkg).await?;
            }
            result.removed.push(pkg.as_ref().clone());
        }

        // Phase 2: Process updates in parallel
        let update_results: Vec<_> = stream::iter(updates.iter())
            .map(|(from, to)| {
                let library_installer = self.library_installer.clone();
                let binary_installer = self.binary_installer.clone();
                async move {
                    // Handle metapackage transitions
                    if to.is_metapackage() {
                        if !from.is_metapackage() {
                            binary_installer.uninstall(from).await?;
                            library_installer.uninstall(from).await?;
                        }
                        // Metapackages have no files to install
                        return Ok::<_, crate::ComposerError>((from.clone(), to.clone(), Vec::new()));
                    }

                    if from.is_metapackage() {
                        // Downgrading from metapackage to regular
                        library_installer.install(to).await?;
                    } else {
                        // Regular update
                        library_installer.update(from, to).await?;
                        binary_installer.uninstall(from).await?;
                    }
                    let bins = binary_installer.install(to).await?;
                    Ok((from.clone(), to.clone(), bins))
                }
            })
            .buffer_unordered(MAX_CONCURRENT_INSTALLS)
            .collect()
            .await;

        for update_result in update_results {
            let (from, to, bins) = update_result?;
            result.updated.push((from.as_ref().clone(), to.as_ref().clone()));
            result.binaries.extend(bins);
        }

        // Phase 3: Process installs in parallel
        let install_results: Vec<_> = stream::iter(installs.iter())
            .map(|pkg| {
                let library_installer = self.library_installer.clone();
                let binary_installer = self.binary_installer.clone();
                async move {
                    if pkg.is_metapackage() {
                        // Metapackages have no files
                        return Ok::<_, crate::ComposerError>((pkg.clone(), Vec::new(), false));
                    }

                    let download_result = library_installer.install(pkg).await?;
                    if download_result.skipped {
                        return Ok((pkg.clone(), Vec::new(), true));
                    }
                    let bins = binary_installer.install(pkg).await?;
                    Ok((pkg.clone(), bins, false))
                }
            })
            .buffer_unordered(MAX_CONCURRENT_INSTALLS)
            .collect()
            .await;

        for install_result in install_results {
            let (pkg, bins, skipped) = install_result?;
            if !skipped {
                result.installed.push(pkg.as_ref().clone());
                result.binaries.extend(bins);
            }
        }

        Ok(result)
    }

    /// Uninstall a package
    async fn uninstall_package(&self, package: &Package) -> Result<()> {
        self.library_installer.uninstall(package).await
    }

    /// Install from a list of packages (without a transaction)
    pub async fn install_packages(&self, packages: &[Package]) -> Result<InstallResult> {
        let mut result = InstallResult {
            installed: Vec::new(),
            updated: Vec::new(),
            removed: Vec::new(),
            binaries: Vec::new(),
        };

        if self.config.dry_run {
            result.installed = packages.to_vec();
            return Ok(result);
        }

        // Create vendor directory
        tokio::fs::create_dir_all(&self.config.vendor_dir).await?;

        // Filter out platform packages and separate metapackages
        let mut metapackages = Vec::new();
        let mut regular_packages = Vec::new();

        for package in packages {
            if package.is_platform_package() {
                continue;
            }
            if package.is_metapackage() {
                metapackages.push(package);
            } else {
                regular_packages.push(package);
            }
        }

        // Handle metapackages (no files, quick)
        for package in metapackages {
            self.metapackage_installer.install(package).await?;
            result.installed.push(package.clone());
        }

        // Install regular packages in parallel
        let install_results: Vec<_> = stream::iter(regular_packages.iter())
            .map(|package| {
                let library_installer = self.library_installer.clone();
                let binary_installer = self.binary_installer.clone();
                async move {
                    library_installer.install(package).await?;
                    let bins = binary_installer.install(package).await?;
                    Ok::<_, crate::ComposerError>(((*package).clone(), bins))
                }
            })
            .buffer_unordered(MAX_CONCURRENT_INSTALLS)
            .collect()
            .await;

        for install_result in install_results {
            let (pkg, bins) = install_result?;
            result.installed.push(pkg);
            result.binaries.extend(bins);
        }

        Ok(result)
    }

    /// Get the config
    pub fn config(&self) -> &InstallConfig {
        &self.config
    }
}

/// Helper module for cache directory
mod dirs {
    use std::path::PathBuf;

    pub fn cache_dir() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/composer"))
        }

        #[cfg(target_os = "linux")]
        {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
                .map(|p| p.join("composer"))
        }

        #[cfg(target_os = "windows")]
        {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|p| p.join("Composer"))
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".composer/cache"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_config_default() {
        let config = InstallConfig::default();
        assert_eq!(config.vendor_dir, PathBuf::from("vendor"));
        assert_eq!(config.bin_dir, PathBuf::from("vendor/bin"));
        assert!(config.prefer_dist);
        assert!(!config.prefer_source);
        assert!(!config.dry_run);
    }

    #[tokio::test]
    async fn test_installation_manager_creation() {
        let http_client = Arc::new(HttpClient::new().unwrap());
        let config = InstallConfig::default();
        let _manager = InstallationManager::new(http_client, config);
    }

    #[tokio::test]
    async fn test_dry_run_install() {
        let http_client = Arc::new(HttpClient::new().unwrap());
        let config = InstallConfig {
            dry_run: true,
            ..Default::default()
        };
        let manager = InstallationManager::new(http_client, config);

        let packages = vec![
            Package::new("vendor/a", "1.0.0"),
            Package::new("vendor/b", "2.0.0"),
        ];

        let result = manager.install_packages(&packages).await.unwrap();
        assert_eq!(result.installed.len(), 2);
        assert!(result.updated.is_empty());
        assert!(result.removed.is_empty());
    }
}
