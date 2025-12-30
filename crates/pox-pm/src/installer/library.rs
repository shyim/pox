//! Library installer - installs packages to vendor directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::downloader::{DownloadManager, DownloadResult};
use crate::package::Package;
use crate::Result;

/// Library installer for standard Composer packages
pub struct LibraryInstaller {
    download_manager: Arc<DownloadManager>,
    vendor_dir: PathBuf,
}

impl LibraryInstaller {
    /// Create a new library installer
    pub fn new(download_manager: Arc<DownloadManager>, vendor_dir: impl Into<PathBuf>) -> Self {
        Self {
            download_manager,
            vendor_dir: vendor_dir.into(),
        }
    }

    /// Get the install path for a package
    pub fn get_install_path(&self, package: &Package) -> PathBuf {
        self.vendor_dir.join(&package.name)
    }

    /// Check if a package is installed
    pub fn is_installed(&self, package: &Package) -> bool {
        let install_path = self.get_install_path(package);
        install_path.exists()
    }

    /// Install a package
    ///
    /// If the package is already installed, this is a no-op and returns Ok with skipped flag.
    pub async fn install(&self, package: &Package) -> Result<DownloadResult> {
        let install_path = self.get_install_path(package);

        // Check if already installed - skip if so
        if install_path.exists() {
            return Ok(DownloadResult {
                path: install_path,
                from_cache: false,
                skipped: true,
            });
        }

        // Download and extract
        self.download_manager.download(package).await
    }

    /// Update a package
    pub async fn update(&self, from: &Package, to: &Package) -> Result<DownloadResult> {
        // Remove old version
        self.uninstall(from).await?;

        // Install new version
        self.download_manager.download(to).await
    }

    /// Uninstall a package
    pub async fn uninstall(&self, package: &Package) -> Result<()> {
        let install_path = self.get_install_path(package);

        if install_path.exists() {
            tokio::fs::remove_dir_all(&install_path).await?;
        }

        Ok(())
    }

    /// Get the vendor directory
    pub fn vendor_dir(&self) -> &Path {
        &self.vendor_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::DownloadConfig;
    use crate::http::HttpClient;
    use tempfile::TempDir;

    fn create_test_installer() -> (LibraryInstaller, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let http_client = Arc::new(HttpClient::new().unwrap());
        let download_config = DownloadConfig {
            vendor_dir: temp_dir.path().join("vendor"),
            cache_dir: temp_dir.path().join("cache"),
            ..Default::default()
        };
        let download_manager = Arc::new(DownloadManager::new(http_client, download_config));
        let installer = LibraryInstaller::new(
            download_manager,
            temp_dir.path().join("vendor"),
        );
        (installer, temp_dir)
    }

    #[test]
    fn test_get_install_path() {
        let (installer, _temp) = create_test_installer();
        let package = Package::new("vendor/package", "1.0.0");

        let path = installer.get_install_path(&package);
        assert!(path.ends_with("vendor/vendor/package"));
    }

    #[test]
    fn test_is_not_installed() {
        let (installer, _temp) = create_test_installer();
        let package = Package::new("vendor/package", "1.0.0");

        assert!(!installer.is_installed(&package));
    }

    #[tokio::test]
    async fn test_uninstall_nonexistent() {
        let (installer, _temp) = create_test_installer();
        let package = Package::new("vendor/package", "1.0.0");

        // Should not error when uninstalling non-existent package
        let result = installer.uninstall(&package).await;
        assert!(result.is_ok());
    }
}
