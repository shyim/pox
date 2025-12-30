use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Source information for a package (VCS like git, hg, svn)
///
/// The source represents where the package source code can be obtained from
/// version control systems.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Type of source repository (git, hg, svn, etc.)
    #[serde(rename = "type")]
    pub source_type: String,

    /// URL to the repository
    pub url: String,

    /// Reference (commit hash, tag, branch) to check out
    pub reference: String,

    /// Mirror URLs for this source (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirrors: Option<Vec<Mirror>>,
}

impl Source {
    /// Creates a new source configuration
    pub fn new(
        source_type: impl Into<String>,
        url: impl Into<String>,
        reference: impl Into<String>,
    ) -> Self {
        Self {
            source_type: source_type.into(),
            url: url.into(),
            reference: reference.into(),
            mirrors: None,
        }
    }

    /// Creates a git source
    pub fn git(url: impl Into<String>, reference: impl Into<String>) -> Self {
        Self::new("git", url, reference)
    }

    /// Creates a mercurial (hg) source
    pub fn hg(url: impl Into<String>, reference: impl Into<String>) -> Self {
        Self::new("hg", url, reference)
    }

    /// Creates an svn source
    pub fn svn(url: impl Into<String>, reference: impl Into<String>) -> Self {
        Self::new("svn", url, reference)
    }

    /// Adds mirror URLs
    pub fn with_mirrors(mut self, mirrors: Vec<Mirror>) -> Self {
        self.mirrors = Some(mirrors);
        self
    }

    /// Returns all URLs (primary + mirrors) ordered by preference
    pub fn urls(&self) -> Vec<String> {
        let mut urls = vec![self.url.clone()];

        if let Some(mirrors) = &self.mirrors {
            for mirror in mirrors {
                if mirror.preferred {
                    urls.insert(0, mirror.url.clone());
                } else {
                    urls.push(mirror.url.clone());
                }
            }
        }

        urls
    }
}

impl Default for Source {
    fn default() -> Self {
        Self {
            source_type: "git".to_string(),
            url: String::new(),
            reference: String::new(),
            mirrors: None,
        }
    }
}

/// Distribution information for a package (archive download)
///
/// The dist represents where a pre-packaged archive of the package can be downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dist {
    /// Type of distribution archive (zip, tar, etc.)
    #[serde(rename = "type")]
    pub dist_type: String,

    /// URL to download the archive
    pub url: String,

    /// Reference (usually same as version or commit hash)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,

    /// SHA-1 checksum of the archive
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shasum: Option<String>,

    /// SHA-256 checksum of the archive (newer packages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    /// Mirror URLs for this distribution (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirrors: Option<Vec<Mirror>>,

    /// Transport options (used for path repositories: symlink, relative)
    #[serde(rename = "transport-options", skip_serializing_if = "Option::is_none")]
    pub transport_options: Option<std::collections::HashMap<String, Value>>,
}

impl Dist {
    /// Creates a new distribution configuration
    pub fn new(dist_type: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            dist_type: dist_type.into(),
            url: url.into(),
            reference: None,
            shasum: None,
            sha256: None,
            mirrors: None,
            transport_options: None,
        }
    }

    /// Creates a zip distribution
    pub fn zip(url: impl Into<String>) -> Self {
        Self::new("zip", url)
    }

    /// Creates a tar distribution
    pub fn tar(url: impl Into<String>) -> Self {
        Self::new("tar", url)
    }

    /// Creates a path distribution (for local path repositories)
    pub fn path(url: impl Into<String>) -> Self {
        Self::new("path", url)
    }

    /// Sets the reference
    pub fn with_reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = Some(reference.into());
        self
    }

    /// Sets the SHA-1 checksum
    pub fn with_shasum(mut self, shasum: impl Into<String>) -> Self {
        self.shasum = Some(shasum.into());
        self
    }

    /// Sets the SHA-256 checksum
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }

    /// Adds mirror URLs
    pub fn with_mirrors(mut self, mirrors: Vec<Mirror>) -> Self {
        self.mirrors = Some(mirrors);
        self
    }

    /// Sets transport options (for path distributions)
    pub fn with_transport_options(mut self, options: std::collections::HashMap<String, Value>) -> Self {
        self.transport_options = Some(options);
        self
    }

    /// Returns all URLs (primary + mirrors) ordered by preference
    pub fn urls(&self) -> Vec<String> {
        let mut urls = vec![self.url.clone()];

        if let Some(mirrors) = &self.mirrors {
            for mirror in mirrors {
                if mirror.preferred {
                    urls.insert(0, mirror.url.clone());
                } else {
                    urls.push(mirror.url.clone());
                }
            }
        }

        urls
    }
}

impl Default for Dist {
    fn default() -> Self {
        Self {
            dist_type: "zip".to_string(),
            url: String::new(),
            reference: None,
            shasum: None,
            sha256: None,
            mirrors: None,
            transport_options: None,
        }
    }
}

/// Mirror configuration for source or dist
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mirror {
    /// Mirror URL
    pub url: String,
    /// Whether this mirror should be preferred over the primary URL
    pub preferred: bool,
}

impl Mirror {
    /// Creates a new mirror
    pub fn new(url: impl Into<String>, preferred: bool) -> Self {
        Self {
            url: url.into(),
            preferred,
        }
    }

    /// Creates a preferred mirror
    pub fn preferred(url: impl Into<String>) -> Self {
        Self::new(url, true)
    }

    /// Creates a fallback mirror
    pub fn fallback(url: impl Into<String>) -> Self {
        Self::new(url, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_git() {
        let source = Source::git(
            "https://github.com/example/repo.git",
            "abc123",
        );

        assert_eq!(source.source_type, "git");
        assert_eq!(source.url, "https://github.com/example/repo.git");
        assert_eq!(source.reference, "abc123");
    }

    #[test]
    fn test_source_with_mirrors() {
        let mirrors = vec![
            Mirror::preferred("https://mirror1.example.com/repo.git"),
            Mirror::fallback("https://mirror2.example.com/repo.git"),
        ];

        let source = Source::git("https://github.com/example/repo.git", "abc123")
            .with_mirrors(mirrors);

        let urls = source.urls();
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://mirror1.example.com/repo.git"); // preferred first
        assert_eq!(urls[1], "https://github.com/example/repo.git"); // original
        assert_eq!(urls[2], "https://mirror2.example.com/repo.git"); // fallback last
    }

    #[test]
    fn test_dist_zip() {
        let dist = Dist::zip("https://example.com/package.zip")
            .with_reference("1.0.0")
            .with_shasum("abc123def456");

        assert_eq!(dist.dist_type, "zip");
        assert_eq!(dist.url, "https://example.com/package.zip");
        assert_eq!(dist.reference, Some("1.0.0".to_string()));
        assert_eq!(dist.shasum, Some("abc123def456".to_string()));
    }

    #[test]
    fn test_dist_with_mirrors() {
        let mirrors = vec![
            Mirror::preferred("https://mirror1.example.com/package.zip"),
            Mirror::fallback("https://mirror2.example.com/package.zip"),
        ];

        let dist = Dist::zip("https://example.com/package.zip")
            .with_mirrors(mirrors);

        let urls = dist.urls();
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://mirror1.example.com/package.zip");
        assert_eq!(urls[1], "https://example.com/package.zip");
        assert_eq!(urls[2], "https://mirror2.example.com/package.zip");
    }
}
