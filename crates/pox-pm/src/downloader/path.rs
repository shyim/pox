//! Path downloader - installs packages from local paths using symlinks or mirroring.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::Result;
use crate::ComposerError;

/// Installation strategy for path packages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStrategy {
    /// Create a symlink to the source directory
    Symlink,
    /// Mirror (copy) files to the destination
    Mirror,
}

impl Default for PathStrategy {
    fn default() -> Self {
        Self::Symlink
    }
}

/// Path downloader for installing packages from local paths
pub struct PathDownloader {
    /// Default strategy
    default_strategy: PathStrategy,
}

impl PathDownloader {
    /// Create a new path downloader
    pub fn new() -> Self {
        Self {
            default_strategy: PathStrategy::Symlink,
        }
    }

    /// Create a path downloader with a specific default strategy
    pub fn with_strategy(strategy: PathStrategy) -> Self {
        Self {
            default_strategy: strategy,
        }
    }

    /// Install a package from a local path
    ///
    /// # Arguments
    /// * `source` - Source path (the package location)
    /// * `dest` - Destination path (where to install)
    /// * `strategy` - Optional strategy override
    /// * `relative` - Whether to use relative symlinks
    pub fn install(
        &self,
        source: &Path,
        dest: &Path,
        strategy: Option<PathStrategy>,
        relative: bool,
    ) -> Result<PathInstallResult> {
        let strategy = strategy.unwrap_or(self.default_strategy);

        // Ensure source exists
        if !source.exists() {
            return Err(ComposerError::DownloadFailed {
                package: source.to_string_lossy().to_string(),
                reason: "Source path does not exist".to_string(),
            });
        }

        // Remove destination if it exists
        if dest.exists() {
            if dest.is_symlink() || dest.is_file() {
                std::fs::remove_file(dest)?;
            } else {
                std::fs::remove_dir_all(dest)?;
            }
        }

        // Ensure parent directory exists
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        match strategy {
            PathStrategy::Symlink => {
                self.create_symlink(source, dest, relative)?;
                Ok(PathInstallResult {
                    path: dest.to_path_buf(),
                    strategy: PathStrategy::Symlink,
                    relative,
                })
            }
            PathStrategy::Mirror => {
                self.mirror_directory(source, dest)?;
                Ok(PathInstallResult {
                    path: dest.to_path_buf(),
                    strategy: PathStrategy::Mirror,
                    relative: false,
                })
            }
        }
    }

    /// Create a symlink from dest to source
    fn create_symlink(&self, source: &Path, dest: &Path, relative: bool) -> Result<()> {
        let link_target = if relative {
            // Calculate relative path from dest to source
            Self::relative_path(dest, source)?
        } else {
            // Use absolute path
            source.canonicalize().map_err(|e| ComposerError::Io(e))?
        };

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&link_target, dest)?;
        }

        #[cfg(windows)]
        {
            // On Windows, use directory junction for better compatibility
            if source.is_dir() {
                std::os::windows::fs::symlink_dir(&link_target, dest)?;
            } else {
                std::os::windows::fs::symlink_file(&link_target, dest)?;
            }
        }

        Ok(())
    }

    /// Calculate relative path from `from` to `to`
    fn relative_path(from: &Path, to: &Path) -> Result<PathBuf> {
        let from_abs = from.parent()
            .ok_or_else(|| ComposerError::DownloadFailed {
                package: from.to_string_lossy().to_string(),
                reason: "Cannot get parent directory".to_string(),
            })?
            .canonicalize()
            .unwrap_or_else(|_| from.parent().unwrap().to_path_buf());

        let to_abs = to.canonicalize()
            .unwrap_or_else(|_| to.to_path_buf());

        // Find common prefix
        let from_components: Vec<_> = from_abs.components().collect();
        let to_components: Vec<_> = to_abs.components().collect();

        let mut common_len = 0;
        for (a, b) in from_components.iter().zip(to_components.iter()) {
            if a == b {
                common_len += 1;
            } else {
                break;
            }
        }

        // Build relative path
        let mut relative = PathBuf::new();

        // Go up from `from` to common ancestor
        for _ in common_len..from_components.len() {
            relative.push("..");
        }

        // Go down from common ancestor to `to`
        for component in &to_components[common_len..] {
            relative.push(component);
        }

        Ok(relative)
    }

    /// Mirror (copy) a directory
    fn mirror_directory(&self, source: &Path, dest: &Path) -> Result<()> {
        std::fs::create_dir_all(dest)?;

        for entry in WalkDir::new(source)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let relative = path.strip_prefix(source).unwrap_or(path);
            let target = dest.join(relative);

            if path.is_dir() {
                std::fs::create_dir_all(&target)?;
            } else if path.is_file() {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(path, &target)?;
            }
            // Skip symlinks and other special files
        }

        Ok(())
    }

    /// Update a package (re-install with same settings)
    pub fn update(
        &self,
        source: &Path,
        dest: &Path,
        strategy: Option<PathStrategy>,
        relative: bool,
    ) -> Result<PathInstallResult> {
        // For path packages, update is the same as install
        self.install(source, dest, strategy, relative)
    }

    /// Remove an installed package
    pub fn remove(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        if path.is_symlink() {
            std::fs::remove_file(path)?;
        } else if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }

        Ok(())
    }

    /// Check if a path is a symlink
    pub fn is_symlink(path: &Path) -> bool {
        path.is_symlink()
    }
}

impl Default for PathDownloader {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a path installation
#[derive(Debug)]
pub struct PathInstallResult {
    /// Path where the package was installed
    pub path: PathBuf,
    /// Strategy used for installation
    pub strategy: PathStrategy,
    /// Whether relative symlink was used
    pub relative: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_package(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("composer.json"), r#"{"name": "test/pkg"}"#).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/Test.php"), "<?php class Test {}").unwrap();
    }

    #[test]
    fn test_symlink_install() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");

        create_test_package(&source);

        let downloader = PathDownloader::new();
        let result = downloader.install(&source, &dest, Some(PathStrategy::Symlink), false).unwrap();

        assert_eq!(result.strategy, PathStrategy::Symlink);
        assert!(dest.exists());
        assert!(dest.is_symlink());
        assert!(dest.join("composer.json").exists());
    }

    #[test]
    fn test_mirror_install() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");

        create_test_package(&source);

        let downloader = PathDownloader::new();
        let result = downloader.install(&source, &dest, Some(PathStrategy::Mirror), false).unwrap();

        assert_eq!(result.strategy, PathStrategy::Mirror);
        assert!(dest.exists());
        assert!(!dest.is_symlink());
        assert!(dest.join("composer.json").exists());
        assert!(dest.join("src/Test.php").exists());
    }

    #[test]
    fn test_relative_symlink() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("packages/source");
        let dest = temp.path().join("vendor/test/pkg");

        create_test_package(&source);

        let downloader = PathDownloader::new();
        let result = downloader.install(&source, &dest, Some(PathStrategy::Symlink), true).unwrap();

        assert!(result.relative);
        assert!(dest.exists());
        assert!(dest.is_symlink());
    }

    #[test]
    fn test_remove() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");

        create_test_package(&source);

        let downloader = PathDownloader::new();
        downloader.install(&source, &dest, Some(PathStrategy::Symlink), false).unwrap();

        assert!(dest.exists());

        downloader.remove(&dest).unwrap();

        assert!(!dest.exists());
    }
}
