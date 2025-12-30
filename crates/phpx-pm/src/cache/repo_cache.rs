//! Repository cache with HTTP metadata support (Last-Modified, ETag)

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::Cache;

/// Cache entry metadata stored alongside cached content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    /// HTTP Last-Modified header value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// HTTP ETag header value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

impl Default for CacheMetadata {
    fn default() -> Self {
        Self {
            last_modified: None,
            etag: None,
        }
    }
}

/// Repository cache that stores metadata alongside cached content
///
/// This cache stores two files for each entry:
/// - `<key>` - The actual cached content
/// - `<key>.meta` - JSON metadata (Last-Modified, ETag, etc.)
pub struct RepoCache {
    /// Underlying filesystem cache
    cache: Cache,
}

impl RepoCache {
    /// Create a new repository cache
    ///
    /// # Arguments
    /// * `cache_dir` - Base cache directory
    /// * `repo_url` - Repository URL (used to create unique cache subdirectory)
    pub fn new(cache_dir: PathBuf, repo_url: &str) -> Self {
        // Sanitize repo URL to create cache subdirectory
        let sanitized = Self::sanitize_url(repo_url);
        let cache_path = cache_dir.join("repo").join(sanitized);

        Self {
            cache: Cache::new(cache_path),
        }
    }

    /// Sanitize a URL for use as a directory name
    fn sanitize_url(url: &str) -> String {
        // Remove protocol
        let url = url
            .trim_start_matches("https://")
            .trim_start_matches("http://");

        // Replace non-alphanumeric characters with dashes
        let re = regex::Regex::new(r"[^a-zA-Z0-9]").unwrap();
        re.replace_all(url, "-").to_lowercase()
    }

    /// Set read-only mode
    pub fn set_read_only(&mut self, read_only: bool) {
        self.cache.set_read_only(read_only);
    }

    /// Check if cache is enabled
    pub fn is_enabled(&self) -> bool {
        self.cache.is_enabled()
    }

    /// Check if cache is read-only
    pub fn is_read_only(&self) -> bool {
        self.cache.is_read_only()
    }

    /// Get the metadata key for a cache key
    fn meta_key(key: &str) -> String {
        format!("{}.meta", key)
    }

    /// Read cached content with metadata
    ///
    /// # Returns
    /// Tuple of (content, metadata) if cached, None otherwise
    pub fn read(&self, key: &str) -> io::Result<Option<(Vec<u8>, CacheMetadata)>> {
        // Read main content
        let content = match self.cache.read(key)? {
            Some(c) => c,
            None => return Ok(None),
        };

        // Read metadata (optional)
        let metadata = match self.cache.read(&Self::meta_key(key))? {
            Some(meta_bytes) => {
                serde_json::from_slice(&meta_bytes).unwrap_or_default()
            }
            None => CacheMetadata::default(),
        };

        Ok(Some((content, metadata)))
    }

    /// Read only the metadata for a cache key
    pub fn read_metadata(&self, key: &str) -> io::Result<Option<CacheMetadata>> {
        match self.cache.read(&Self::meta_key(key))? {
            Some(meta_bytes) => {
                let metadata: CacheMetadata = serde_json::from_slice(&meta_bytes)
                    .unwrap_or_default();
                Ok(Some(metadata))
            }
            None => Ok(None),
        }
    }

    /// Write content with metadata to cache
    pub fn write(&self, key: &str, content: &[u8], metadata: &CacheMetadata) -> io::Result<()> {
        // Write main content
        self.cache.write(key, content)?;

        // Write metadata
        let meta_bytes = serde_json::to_vec(metadata)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.cache.write(&Self::meta_key(key), &meta_bytes)?;

        Ok(())
    }

    /// Check if a cached entry exists
    pub fn has(&self, key: &str) -> bool {
        self.cache.has(key)
    }

    /// Get age of cached entry
    pub fn age(&self, key: &str) -> io::Result<Option<Duration>> {
        self.cache.age(key)
    }

    /// Remove a cached entry
    pub fn remove(&self, key: &str) -> io::Result<()> {
        self.cache.remove(key)?;
        self.cache.remove(&Self::meta_key(key))?;
        Ok(())
    }

    /// Clear all cached entries
    pub fn clear(&self) -> io::Result<()> {
        self.cache.clear()
    }

    /// Garbage collect old entries
    pub fn gc(&self, ttl: Duration) -> io::Result<u64> {
        self.cache.gc(ttl)
    }

    /// Get SHA256 hash of cached content
    pub fn sha256(&self, key: &str) -> io::Result<Option<String>> {
        self.cache.sha256(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_repo_cache_new() {
        let temp = TempDir::new().unwrap();
        let cache = RepoCache::new(temp.path().to_path_buf(), "https://repo.packagist.org");

        assert!(cache.is_enabled());
        assert!(!cache.is_read_only());
    }

    #[test]
    fn test_repo_cache_write_read() {
        let temp = TempDir::new().unwrap();
        let cache = RepoCache::new(temp.path().to_path_buf(), "https://repo.packagist.org");

        let content = b"test content";
        let metadata = CacheMetadata {
            last_modified: Some("Wed, 24 Dec 2025 10:00:00 GMT".to_string()),
            etag: None,
        };

        cache.write("test-key", content, &metadata).unwrap();

        let (read_content, read_metadata) = cache.read("test-key").unwrap().unwrap();
        assert_eq!(read_content, content);
        assert_eq!(read_metadata.last_modified, metadata.last_modified);
    }

    #[test]
    fn test_repo_cache_read_metadata_only() {
        let temp = TempDir::new().unwrap();
        let cache = RepoCache::new(temp.path().to_path_buf(), "https://repo.packagist.org");

        let metadata = CacheMetadata {
            last_modified: Some("Wed, 24 Dec 2025 10:00:00 GMT".to_string()),
            etag: Some("\"abc123\"".to_string()),
        };

        cache.write("test-key", b"content", &metadata).unwrap();

        let read_metadata = cache.read_metadata("test-key").unwrap().unwrap();
        assert_eq!(read_metadata.last_modified, metadata.last_modified);
        assert_eq!(read_metadata.etag, metadata.etag);
    }

    #[test]
    fn test_sanitize_url() {
        assert_eq!(
            RepoCache::sanitize_url("https://repo.packagist.org"),
            "repo-packagist-org"
        );
        assert_eq!(
            RepoCache::sanitize_url("https://packages.example.com/composer"),
            "packages-example-com-composer"
        );
    }
}
