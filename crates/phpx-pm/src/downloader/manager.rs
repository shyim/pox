//! Download manager for orchestrating package downloads.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::http::HttpClient;
use crate::package::{Dist, Source};
use crate::{ComposerError, Package, Result};

use super::archive::ArchiveExtractor;
use super::checksum::{verify_checksum, ChecksumType};
use super::file::FileDownloader;
use super::git::GitDownloader;
use super::path::{PathDownloader, PathStrategy};

/// Result of a download operation
#[derive(Debug)]
pub struct DownloadResult {
    /// Path where the package was extracted
    pub path: PathBuf,
    /// Whether the download was from cache
    pub from_cache: bool,
    /// Whether the download was skipped (already installed)
    pub skipped: bool,
}

/// Configuration for the download manager
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Prefer source over dist
    pub prefer_source: bool,
    /// Prefer dist over source
    pub prefer_dist: bool,
    /// Cache directory for downloaded archives
    pub cache_dir: PathBuf,
    /// Vendor directory for extracted packages
    pub vendor_dir: PathBuf,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            prefer_source: false,
            prefer_dist: true,
            cache_dir: PathBuf::from(".composer/cache"),
            vendor_dir: PathBuf::from("vendor"),
        }
    }
}

/// Download manager for package installation
pub struct DownloadManager {
    file_downloader: FileDownloader,
    git_downloader: GitDownloader,
    path_downloader: PathDownloader,
    config: DownloadConfig,
}

impl DownloadManager {
    /// Create a new download manager
    pub fn new(http_client: Arc<HttpClient>, config: DownloadConfig) -> Self {
        Self {
            file_downloader: FileDownloader::new(http_client),
            git_downloader: GitDownloader::new(),
            path_downloader: PathDownloader::new(),
            config,
        }
    }

    /// Download and install a package
    pub async fn download(&self, package: &Package) -> Result<DownloadResult> {
        let dest_dir = self.package_path(package);

        if let Some(dist) = &package.dist {
            if dist.dist_type == "path" {
                log::debug!("Installing {} ({}) from path", package.name, package.version);
                return self.download_from_path(package, dist, &dest_dir).await;
            }
        }

        let use_source = self.should_use_source(package);

        if use_source {
            if let Some(source) = &package.source {
                log::debug!("Installing {} ({}) from source ({})",
                    package.name, package.version, source.source_type);
                self.download_from_source(package, source, &dest_dir).await?;
                return Ok(DownloadResult {
                    path: dest_dir,
                    from_cache: false,
                    skipped: false,
                });
            }
        }

        // Try dist download
        if let Some(dist) = &package.dist {
            let from_cache = self.download_from_dist(package, dist, &dest_dir).await?;
            if from_cache {
                log::debug!("Loading {} ({}) from cache", package.name, package.version);
            } else {
                log::debug!("Downloading {} ({})", package.name, package.version);
            }
            return Ok(DownloadResult {
                path: dest_dir,
                from_cache,
                skipped: false,
            });
        }

        // Fallback to source if dist not available
        if let Some(source) = &package.source {
            log::debug!("Installing {} ({}) from source ({})",
                package.name, package.version, source.source_type);
            self.download_from_source(package, source, &dest_dir).await?;
            return Ok(DownloadResult {
                path: dest_dir,
                from_cache: false,
                skipped: false,
            });
        }

        Err(ComposerError::DownloadFailed {
            package: package.name.clone(),
            reason: "No source or dist available".to_string(),
        })
    }

    /// Download multiple packages
    pub async fn download_many(&self, packages: &[Package]) -> Vec<Result<DownloadResult>> {
        let mut results = Vec::with_capacity(packages.len());

        for package in packages {
            results.push(self.download(package).await);
        }

        results
    }

    /// Download from dist (archive)
    /// Returns true if the download was from cache
    async fn download_from_dist(
        &self,
        package: &Package,
        dist: &Dist,
        dest_dir: &Path,
    ) -> Result<bool> {
        let cache_file = self.cache_path(package, &dist.dist_type);
        if let Some(parent) = cache_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Try URLs in order (primary + mirrors)
        let urls = dist.urls();

        let checksum = dist.sha256.as_ref()
            .filter(|s| !s.is_empty())
            .or_else(|| dist.shasum.as_ref().filter(|s| !s.is_empty()));

        for url in &urls {
            if cache_file.exists() {
                // Verify checksum if available
                if let Some(checksum) = checksum {
                    let checksum_type = ChecksumType::from_hex_length(checksum.len())
                        .unwrap_or(ChecksumType::Sha256);

                    if verify_checksum(&cache_file, checksum, checksum_type).await? {
                        self.extract_archive(&cache_file, dest_dir)?;
                        return Ok(true);
                    }
                    let _ = tokio::fs::remove_file(&cache_file).await;
                } else {
                    self.extract_archive(&cache_file, dest_dir)?;
                    return Ok(true);
                }
            }

            let result = self
                .file_downloader
                .download(url, &cache_file, None::<fn(u64, u64)>)
                .await;

            if let Err(e) = result {
                eprintln!("Warning: Failed to download from {}: {}", url, e);
                continue;
            }

            // Verify checksum if available
            if let Some(checksum) = checksum {
                let checksum_type = ChecksumType::from_hex_length(checksum.len())
                    .unwrap_or(ChecksumType::Sha256);

                if !verify_checksum(&cache_file, checksum, checksum_type).await? {
                    let _ = tokio::fs::remove_file(&cache_file).await;
                    return Err(ComposerError::ChecksumMismatch {
                        package: package.name.clone(),
                    });
                }
            }

            // Extract the archive
            self.extract_archive(&cache_file, dest_dir)?;
            return Ok(false);
        }

        Err(ComposerError::DownloadFailed {
            package: package.name.clone(),
            reason: "All download URLs failed".to_string(),
        })
    }

    /// Download from source (git)
    async fn download_from_source(
        &self,
        package: &Package,
        source: &Source,
        dest_dir: &Path,
    ) -> Result<()> {
        // Create destination directory
        if let Some(parent) = dest_dir.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        match source.source_type.as_str() {
            "git" => {
                // Try URLs in order
                for url in source.urls() {
                    let result = self.git_downloader.clone(
                        &url,
                        dest_dir,
                        Some(&source.reference),
                    );

                    if result.is_ok() {
                        return Ok(());
                    }
                }

                Err(ComposerError::DownloadFailed {
                    package: package.name.clone(),
                    reason: "Git clone failed for all URLs".to_string(),
                })
            }
            other => Err(ComposerError::DownloadFailed {
                package: package.name.clone(),
                reason: format!("Unsupported source type: {}", other),
            }),
        }
    }

    /// Download from path (local directory)
    async fn download_from_path(
        &self,
        _package: &Package,
        dist: &Dist,
        dest_dir: &Path,
    ) -> Result<DownloadResult> {
        let source_path = PathBuf::from(&dist.url);

        // Determine strategy from transport options
        let strategy = dist.transport_options.as_ref()
            .and_then(|opts| opts.get("symlink"))
            .and_then(|v| v.as_bool())
            .map(|symlink| {
                if symlink {
                    PathStrategy::Symlink
                } else {
                    PathStrategy::Mirror
                }
            });

        let relative = dist.transport_options.as_ref()
            .and_then(|opts| opts.get("relative"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Create parent directory if needed
        if let Some(parent) = dest_dir.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        self.path_downloader.install(&source_path, dest_dir, strategy, relative)?;

        Ok(DownloadResult {
            path: dest_dir.to_path_buf(),
            from_cache: false,
            skipped: false,
        })
    }

    /// Extract an archive to destination
    fn extract_archive(&self, archive_path: &Path, dest_dir: &Path) -> Result<()> {
        // Clean destination if it exists
        if dest_dir.exists() {
            std::fs::remove_dir_all(dest_dir)?;
        }
        std::fs::create_dir_all(dest_dir)?;

        ArchiveExtractor::extract(archive_path, dest_dir)
    }

    /// Get the path where a package should be installed
    fn package_path(&self, package: &Package) -> PathBuf {
        self.config.vendor_dir.join(&package.name)
    }

    /// Get the cache path for a package archive
    fn cache_path(&self, package: &Package, archive_type: &str) -> PathBuf {
        let safe_name = package.name.replace('/', "-");
        let filename = format!("{}-{}.{}", safe_name, package.version, archive_type);
        self.config.cache_dir.join("files").join(&package.name).join(filename)
    }

    /// Determine if source should be used for a package
    fn should_use_source(&self, package: &Package) -> bool {
        // Always use source for dev packages
        if package.is_dev() {
            return true;
        }

        // Use config preference
        if self.config.prefer_source {
            return package.source.is_some();
        }

        false
    }

    /// Remove a package
    pub async fn remove(&self, package: &Package) -> Result<()> {
        let dest_dir = self.package_path(package);

        if dest_dir.exists() {
            tokio::fs::remove_dir_all(&dest_dir).await?;
        }

        Ok(())
    }

    /// Update a package (remove old, install new)
    pub async fn update(&self, old: &Package, new: &Package) -> Result<DownloadResult> {
        // Remove old package
        self.remove(old).await?;

        // Download new package
        self.download(new).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_config_default() {
        let config = DownloadConfig::default();
        assert!(config.prefer_dist);
        assert!(!config.prefer_source);
    }

    #[test]
    fn test_package_path() {
        let client = Arc::new(HttpClient::new().unwrap());
        let config = DownloadConfig {
            vendor_dir: PathBuf::from("/app/vendor"),
            ..Default::default()
        };
        let manager = DownloadManager::new(client, config);

        let package = Package::new("vendor/package", "1.0.0");
        let path = manager.package_path(&package);

        assert_eq!(path, PathBuf::from("/app/vendor/vendor/package"));
    }

    #[test]
    fn test_cache_path() {
        let client = Arc::new(HttpClient::new().unwrap());
        let config = DownloadConfig {
            cache_dir: PathBuf::from("/cache"),
            ..Default::default()
        };
        let manager = DownloadManager::new(client, config);

        let package = Package::new("vendor/package", "1.0.0");
        let path = manager.cache_path(&package, "zip");

        assert_eq!(
            path,
            PathBuf::from("/cache/files/vendor/package/vendor-package-1.0.0.zip")
        );
    }

    #[test]
    fn test_should_use_source_dev() {
        let client = Arc::new(HttpClient::new().unwrap());
        let config = DownloadConfig::default();
        let manager = DownloadManager::new(client, config);

        let mut package = Package::new("vendor/package", "dev-main");
        package.source = Some(Source::git(
            "https://github.com/vendor/package.git",
            "abc123",
        ));

        assert!(manager.should_use_source(&package));
    }

    #[test]
    fn test_should_use_source_prefer_source() {
        let client = Arc::new(HttpClient::new().unwrap());
        let config = DownloadConfig {
            prefer_source: true,
            prefer_dist: false,
            ..Default::default()
        };
        let manager = DownloadManager::new(client, config);

        let mut package = Package::new("vendor/package", "1.0.0");
        package.source = Some(Source::git(
            "https://github.com/vendor/package.git",
            "abc123",
        ));

        assert!(manager.should_use_source(&package));
    }
}
