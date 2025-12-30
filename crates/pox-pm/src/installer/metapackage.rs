//! Metapackage installer - handles packages that have no files.
//!
//! Metapackages are "virtual" packages that only define dependencies.
//! They don't have any files to install, but their dependencies are installed.

use crate::package::Package;
use crate::Result;

/// Metapackage installer
///
/// Metapackages are special packages that contain no code files.
/// They exist purely to define a set of dependencies that should be installed together.
/// Examples include framework distribution packages or curated library bundles.
///
/// The installer handles these packages by:
/// - Returning `None` for install path (no files on disk)
/// - Skipping download operations entirely
/// - Only tracking them in the installed packages list
pub struct MetapackageInstaller;

impl MetapackageInstaller {
    /// Create a new metapackage installer
    pub fn new() -> Self {
        Self
    }

    /// Check if a package is a metapackage
    pub fn supports(package: &Package) -> bool {
        package.package_type == "metapackage"
    }

    /// Get the install path for a metapackage
    ///
    /// Always returns `None` because metapackages have no files to install.
    pub fn get_install_path(&self, _package: &Package) -> Option<std::path::PathBuf> {
        None
    }

    /// Check if a metapackage is "installed"
    ///
    /// Metapackages are considered installed if they are tracked in the repository.
    /// Since we don't have direct access to the repository here, we always return true
    /// (the actual tracking is done at the transaction/repository level).
    pub fn is_installed(&self, _package: &Package) -> bool {
        // Metapackages are always "installed" in the sense that there's nothing to do
        true
    }

    /// Install a metapackage
    ///
    /// This is a no-op since metapackages have no files.
    /// The dependencies are handled separately by the solver/transaction.
    pub async fn install(&self, _package: &Package) -> Result<MetapackageResult> {
        Ok(MetapackageResult { installed: true })
    }

    /// Update a metapackage
    ///
    /// This is a no-op since metapackages have no files.
    pub async fn update(&self, _from: &Package, _to: &Package) -> Result<MetapackageResult> {
        Ok(MetapackageResult { installed: true })
    }

    /// Uninstall a metapackage
    ///
    /// This is a no-op since metapackages have no files to remove.
    pub async fn uninstall(&self, _package: &Package) -> Result<()> {
        Ok(())
    }
}

impl Default for MetapackageInstaller {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a metapackage installation operation
#[derive(Debug)]
pub struct MetapackageResult {
    /// Whether the metapackage was processed
    pub installed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metapackage_installer_creation() {
        let _installer = MetapackageInstaller::new();
    }

    #[test]
    fn test_supports_metapackage() {
        let mut pkg = Package::new("vendor/bundle", "1.0.0");
        pkg.package_type = "metapackage".to_string();
        assert!(MetapackageInstaller::supports(&pkg));

        let library_pkg = Package::new("vendor/library", "1.0.0");
        assert!(!MetapackageInstaller::supports(&library_pkg));
    }

    #[test]
    fn test_get_install_path_is_none() {
        let installer = MetapackageInstaller::new();
        let mut pkg = Package::new("vendor/bundle", "1.0.0");
        pkg.package_type = "metapackage".to_string();

        assert!(installer.get_install_path(&pkg).is_none());
    }

    #[test]
    fn test_is_installed() {
        let installer = MetapackageInstaller::new();
        let mut pkg = Package::new("vendor/bundle", "1.0.0");
        pkg.package_type = "metapackage".to_string();

        assert!(installer.is_installed(&pkg));
    }

    #[tokio::test]
    async fn test_install_metapackage() {
        let installer = MetapackageInstaller::new();
        let mut pkg = Package::new("vendor/bundle", "1.0.0");
        pkg.package_type = "metapackage".to_string();

        let result = installer.install(&pkg).await.unwrap();
        assert!(result.installed);
    }

    #[tokio::test]
    async fn test_update_metapackage() {
        let installer = MetapackageInstaller::new();
        let mut from = Package::new("vendor/bundle", "1.0.0");
        from.package_type = "metapackage".to_string();
        let mut to = Package::new("vendor/bundle", "2.0.0");
        to.package_type = "metapackage".to_string();

        let result = installer.update(&from, &to).await.unwrap();
        assert!(result.installed);
    }

    #[tokio::test]
    async fn test_uninstall_metapackage() {
        let installer = MetapackageInstaller::new();
        let mut pkg = Package::new("vendor/bundle", "1.0.0");
        pkg.package_type = "metapackage".to_string();

        let result = installer.uninstall(&pkg).await;
        assert!(result.is_ok());
    }
}
