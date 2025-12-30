//! File downloader for HTTP/HTTPS archives.

use std::path::Path;
use std::sync::Arc;

use crate::http::HttpClient;
use crate::{ComposerError, Result};

use super::archive::{ArchiveExtractor, ArchiveType};
use super::checksum::{verify_checksum, ChecksumType};

/// File downloader for HTTP archives
pub struct FileDownloader {
    http_client: Arc<HttpClient>,
}

impl FileDownloader {
    /// Create a new file downloader
    pub fn new(http_client: Arc<HttpClient>) -> Self {
        Self { http_client }
    }

    /// Download a file to the specified path
    pub async fn download<F>(
        &self,
        url: &str,
        dest: &Path,
        progress: Option<F>,
    ) -> Result<()>
    where
        F: Fn(u64, u64),
    {
        self.http_client
            .download(url, dest, progress)
            .await
            .map_err(|e| ComposerError::DownloadFailed {
                package: url.to_string(),
                reason: e.to_string(),
            })
    }

    /// Download and verify checksum
    pub async fn download_verified<F>(
        &self,
        url: &str,
        dest: &Path,
        expected_checksum: &str,
        progress: Option<F>,
    ) -> Result<()>
    where
        F: Fn(u64, u64),
    {
        // Download the file
        self.download(url, dest, progress).await?;

        // Verify checksum
        let checksum_type = ChecksumType::from_hex_length(expected_checksum.len())
            .ok_or_else(|| ComposerError::ChecksumMismatch {
                package: url.to_string(),
            })?;

        let valid = verify_checksum(dest, expected_checksum, checksum_type).await?;

        if !valid {
            // Remove the downloaded file
            let _ = tokio::fs::remove_file(dest).await;
            return Err(ComposerError::ChecksumMismatch {
                package: url.to_string(),
            });
        }

        Ok(())
    }

    /// Download and extract an archive
    pub async fn download_and_extract<F>(
        &self,
        url: &str,
        dest_dir: &Path,
        expected_checksum: Option<&str>,
        progress: Option<F>,
    ) -> Result<()>
    where
        F: Fn(u64, u64),
    {
        // Determine archive type from URL
        let archive_type = ArchiveType::from_path(Path::new(url))
            .unwrap_or(ArchiveType::Zip);

        // Create temp file for download
        let temp_dir = tempfile::tempdir()?;
        let temp_file = temp_dir.path().join(format!(
            "download.{}",
            match archive_type {
                ArchiveType::Zip => "zip",
                ArchiveType::Tar => "tar",
                ArchiveType::TarGz => "tar.gz",
                ArchiveType::TarBz2 => "tar.bz2",
                ArchiveType::TarXz => "tar.xz",
            }
        ));

        // Download
        if let Some(checksum) = expected_checksum {
            self.download_verified(url, &temp_file, checksum, progress).await?;
        } else {
            self.download(url, &temp_file, progress).await?;
        }

        // Extract
        ArchiveExtractor::extract(&temp_file, dest_dir)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_downloader_creation() {
        let client = Arc::new(HttpClient::new().unwrap());
        let _downloader = FileDownloader::new(client);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_download_file() {
        use tempfile::TempDir;

        let client = Arc::new(HttpClient::new().unwrap());
        let downloader = FileDownloader::new(client);

        let temp_dir = TempDir::new().unwrap();
        let dest = temp_dir.path().join("test.bin");

        let result = downloader
            .download("https://httpbin.org/bytes/100", &dest, None::<fn(u64, u64)>)
            .await;

        assert!(result.is_ok());
        assert!(dest.exists());
    }
}
