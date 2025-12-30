//! Binary installer - creates executable links for package binaries.

use std::path::{Path, PathBuf};

use crate::package::Package;
use crate::Result;

/// Binary installer for creating executable links
pub struct BinaryInstaller {
    /// Directory where binaries are linked
    bin_dir: PathBuf,
    /// Vendor directory where packages are installed
    vendor_dir: PathBuf,
}

impl BinaryInstaller {
    /// Create a new binary installer
    pub fn new(bin_dir: impl Into<PathBuf>, vendor_dir: impl Into<PathBuf>) -> Self {
        Self {
            bin_dir: bin_dir.into(),
            vendor_dir: vendor_dir.into(),
        }
    }

    /// Install binaries for a package
    pub async fn install(&self, package: &Package) -> Result<Vec<PathBuf>> {
        if package.bin.is_empty() {
            return Ok(Vec::new());
        }

        tokio::fs::create_dir_all(&self.bin_dir).await?;

        let mut installed = Vec::new();
        let package_dir = self.vendor_dir.join(&package.name);

        for bin_path in &package.bin {
            let source = package_dir.join(bin_path);
            let bin_name = Path::new(bin_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| bin_path.clone());

            let link_name = bin_name.strip_suffix(".php").unwrap_or(&bin_name);
            let link_path = self.bin_dir.join(link_name);

            if source.exists() {
                self.create_bin_link(&source, &link_path).await?;
                installed.push(link_path);
            }
        }

        Ok(installed)
    }

    /// Remove binaries for a package
    pub async fn uninstall(&self, package: &Package) -> Result<()> {
        for bin_path in &package.bin {
            let bin_name = Path::new(bin_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| bin_path.clone());

            let link_name = bin_name.strip_suffix(".php").unwrap_or(&bin_name);
            let link_path = self.bin_dir.join(link_name);

            if link_path.exists() {
                tokio::fs::remove_file(&link_path).await?;
            }
        }

        Ok(())
    }

    /// Create a binary link (symlink on Unix, batch file on Windows)
    #[cfg(unix)]
    async fn create_bin_link(&self, source: &Path, link: &Path) -> Result<()> {
        if link.exists() {
            tokio::fs::remove_file(link).await?;
        }

        tokio::fs::symlink(source, link).await?;

        use std::os::unix::fs::PermissionsExt;
        let metadata = tokio::fs::metadata(source).await?;
        let mut perms = metadata.permissions();
        perms.set_mode(perms.mode() | 0o111);
        tokio::fs::set_permissions(source, perms).await?;

        Ok(())
    }

    /// Create a binary link (batch file on Windows)
    #[cfg(windows)]
    async fn create_bin_link(&self, source: &Path, link: &Path) -> Result<()> {
        let bat_path = link.with_extension("bat");
        if bat_path.exists() {
            tokio::fs::remove_file(&bat_path).await?;
        }

        let source_str = source.to_string_lossy();
        let content = format!(
            "@ECHO OFF\r\nphp \"{}\" %*\r\n",
            source_str.replace('/', "\\")
        );

        tokio::fs::write(&bat_path, content).await?;

        Ok(())
    }

    /// Get the bin directory
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_binary_installer_creation() {
        let installer = BinaryInstaller::new("/app/vendor/bin", "/app/vendor");
        assert_eq!(installer.bin_dir(), Path::new("/app/vendor/bin"));
    }

    #[tokio::test]
    async fn test_install_no_binaries() {
        let temp_dir = TempDir::new().unwrap();
        let installer = BinaryInstaller::new(
            temp_dir.path().join("bin"),
            temp_dir.path().join("vendor"),
        );

        let package = Package::new("vendor/package", "1.0.0");
        let result = installer.install(&package).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_uninstall_no_binaries() {
        let temp_dir = TempDir::new().unwrap();
        let installer = BinaryInstaller::new(
            temp_dir.path().join("bin"),
            temp_dir.path().join("vendor"),
        );

        let package = Package::new("vendor/package", "1.0.0");
        let result = installer.uninstall(&package).await;

        assert!(result.is_ok());
    }
}
