//! PHP Version Manager
//!
//! This module handles downloading and managing PHP runtime libraries
//! for dynamic version selection.

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Represents a parsed PHP version
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: Option<u32>,
}

impl PhpVersion {
    /// Parse a version string like "8.3", "8.3.15", "^8.2"
    pub fn parse(version_str: &str) -> Result<Self> {
        // Remove constraint prefixes
        let version = version_str
            .trim_start_matches('^')
            .trim_start_matches('~')
            .trim_start_matches(">=")
            .trim_start_matches('>')
            .trim();

        let parts: Vec<&str> = version.split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return Err(anyhow!("Invalid PHP version: {}", version_str));
        }

        let major = parts[0]
            .parse()
            .with_context(|| format!("Invalid major version in: {}", version_str))?;

        let minor = if parts.len() > 1 {
            parts[1]
                .parse()
                .with_context(|| format!("Invalid minor version in: {}", version_str))?
        } else {
            0
        };

        let patch = if parts.len() > 2 {
            Some(
                parts[2]
                    .parse()
                    .with_context(|| format!("Invalid patch version in: {}", version_str))?,
            )
        } else {
            None
        };

        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    /// Convert to version ID (like PHP_VERSION_ID)
    pub fn version_id(&self) -> u32 {
        self.major * 10000 + self.minor * 100 + self.patch.unwrap_or(0)
    }

    /// Format as a string
    pub fn to_string_full(&self) -> String {
        if let Some(patch) = self.patch {
            format!("{}.{}.{}", self.major, self.minor, patch)
        } else {
            format!("{}.{}", self.major, self.minor)
        }
    }

    /// Format major.minor only
    pub fn to_string_short(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}

impl std::fmt::Display for PhpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_full())
    }
}

/// Get the current platform identifier
pub fn get_platform() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    // Detect musl vs glibc on Linux
    let os_variant = if os == "linux" {
        // Check if we're on musl by looking at the executable
        if is_musl() {
            "linux-musl"
        } else {
            "linux-gnu"
        }
    } else {
        os
    };

    format!("{}-{}", os_variant, arch)
}

/// Check if running on musl libc
fn is_musl() -> bool {
    // Check /proc/self/exe linkage or /etc/os-release
    if let Ok(content) = fs::read_to_string("/etc/os-release") {
        if content.contains("Alpine") || content.contains("alpine") {
            return true;
        }
    }

    // Check ldd output for musl
    if let Ok(output) = std::process::Command::new("ldd")
        .arg("--version")
        .output()
    {
        let output_str = String::from_utf8_lossy(&output.stdout);
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if output_str.contains("musl") || stderr_str.contains("musl") {
            return true;
        }
    }

    false
}

/// Get the phpx data directory (for storing downloaded versions)
pub fn get_phpx_home() -> PathBuf {
    // Check PHPX_HOME environment variable first
    if let Ok(home) = std::env::var("PHPX_HOME") {
        return PathBuf::from(home);
    }

    // Default to ~/.phpx
    if let Some(home) = dirs::home_dir() {
        return home.join(".phpx");
    }

    // Fallback to /tmp/.phpx
    PathBuf::from("/tmp/.phpx")
}

/// Get the versions directory
pub fn get_versions_dir() -> PathBuf {
    get_phpx_home().join("versions")
}

/// Get the path for a specific PHP version library
pub fn get_version_lib_path(version: &PhpVersion, platform: &str) -> PathBuf {
    let version_dir = format!("{}-{}", version.to_string_full(), platform);
    let lib_name = if cfg!(target_os = "macos") {
        "libphpx.dylib"
    } else if cfg!(target_os = "windows") {
        "phpx.dll"
    } else {
        "libphpx.so"
    };

    get_versions_dir().join(version_dir).join(lib_name)
}

/// Check if a version is installed
pub fn is_version_installed(version: &PhpVersion, platform: &str) -> bool {
    get_version_lib_path(version, platform).exists()
}

/// Available PHP versions that can be downloaded
#[derive(Debug, Clone)]
pub struct AvailableVersion {
    pub version: PhpVersion,
    pub download_url: String,
    pub checksum: String,
}

/// Manages PHP version downloads and caching
pub struct VersionManager {
    versions_dir: PathBuf,
    platform: String,
    /// Base URL for downloading PHP libraries (can be overridden for testing)
    download_base_url: String,
}

impl VersionManager {
    /// Create a new version manager
    pub fn new() -> Self {
        Self {
            versions_dir: get_versions_dir(),
            platform: get_platform(),
            // This would be the official phpx releases URL
            download_base_url: "https://github.com/YOUR_ORG/phpx/releases/download".to_string(),
        }
    }

    /// Create with a custom download URL (for testing or self-hosting)
    pub fn with_download_url(download_base_url: String) -> Self {
        Self {
            versions_dir: get_versions_dir(),
            platform: get_platform(),
            download_base_url,
        }
    }

    /// Get the platform identifier
    pub fn platform(&self) -> &str {
        &self.platform
    }

    /// Check if a version is available locally
    pub fn is_installed(&self, version: &PhpVersion) -> bool {
        is_version_installed(version, &self.platform)
    }

    /// Get the library path for a version
    pub fn get_lib_path(&self, version: &PhpVersion) -> PathBuf {
        get_version_lib_path(version, &self.platform)
    }

    /// Ensure a version is installed, downloading if necessary
    pub async fn ensure_version(&self, version: &PhpVersion) -> Result<PathBuf> {
        let lib_path = self.get_lib_path(version);

        if lib_path.exists() {
            return Ok(lib_path);
        }

        // Create versions directory if needed
        let version_dir = lib_path.parent().unwrap();
        fs::create_dir_all(version_dir)
            .with_context(|| format!("Failed to create version directory: {:?}", version_dir))?;

        // Download the library
        self.download_version(version).await?;

        if !lib_path.exists() {
            return Err(anyhow!(
                "Failed to download PHP {} for {}",
                version,
                self.platform
            ));
        }

        Ok(lib_path)
    }

    /// Download a specific PHP version
    async fn download_version(&self, version: &PhpVersion) -> Result<()> {
        let download_url = self.get_download_url(version);
        let lib_path = self.get_lib_path(version);

        eprintln!(
            "Downloading PHP {} for {}...",
            version,
            self.platform
        );

        // Use reqwest for async download
        let client = reqwest::Client::new();
        let response = client
            .get(&download_url)
            .send()
            .await
            .with_context(|| format!("Failed to download from: {}", download_url))?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Download failed with status {}: {}",
                response.status(),
                download_url
            ));
        }

        let bytes = response
            .bytes()
            .await
            .with_context(|| "Failed to read download response")?;

        // The download might be a tarball or the library directly
        // For now, assume it's the library directly
        // TODO: Handle .tar.gz archives
        fs::write(&lib_path, &bytes)
            .with_context(|| format!("Failed to write library to: {:?}", lib_path))?;

        // Make it executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&lib_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&lib_path, perms)?;
        }

        eprintln!("Downloaded PHP {} successfully", version);

        Ok(())
    }

    /// Get the download URL for a version
    fn get_download_url(&self, version: &PhpVersion) -> String {
        let lib_name = if cfg!(target_os = "macos") {
            "libphpx.dylib"
        } else if cfg!(target_os = "windows") {
            "phpx.dll"
        } else {
            "libphpx.so"
        };

        format!(
            "{}/v{}/phpx-{}-{}.tar.gz",
            self.download_base_url,
            version.to_string_full(),
            version.to_string_full(),
            self.platform
        )
    }

    /// List installed versions
    pub fn list_installed(&self) -> Result<Vec<PhpVersion>> {
        let mut versions = Vec::new();

        if !self.versions_dir.exists() {
            return Ok(versions);
        }

        for entry in fs::read_dir(&self.versions_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Parse directory name like "8.3.15-linux-gnu-x86_64"
            if let Some(version_part) = name_str.split('-').next() {
                if let Ok(version) = PhpVersion::parse(version_part) {
                    // Check if the library actually exists
                    if self.is_installed(&version) {
                        versions.push(version);
                    }
                }
            }
        }

        // Sort by version
        versions.sort_by(|a, b| a.version_id().cmp(&b.version_id()));

        Ok(versions)
    }

    /// Find the best matching version for a constraint
    pub fn find_matching_version(&self, constraint: &str) -> Result<Option<PhpVersion>> {
        let requested = PhpVersion::parse(constraint)?;
        let installed = self.list_installed()?;

        // For now, simple matching: find exact or compatible version
        // TODO: Implement proper semver constraint matching

        // Check for exact match first
        if let Some(v) = installed.iter().find(|v| {
            v.major == requested.major
                && v.minor == requested.minor
                && (requested.patch.is_none() || v.patch == requested.patch)
        }) {
            return Ok(Some(v.clone()));
        }

        // Check for compatible version (same major.minor)
        if constraint.starts_with('^') || constraint.starts_with('~') {
            if let Some(v) = installed
                .iter()
                .filter(|v| v.major == requested.major && v.minor >= requested.minor)
                .max_by_key(|v| v.version_id())
            {
                return Ok(Some(v.clone()));
            }
        }

        Ok(None)
    }

    /// Remove an installed version
    pub fn remove_version(&self, version: &PhpVersion) -> Result<()> {
        let version_dir = self
            .get_lib_path(version)
            .parent()
            .unwrap()
            .to_path_buf();

        if version_dir.exists() {
            fs::remove_dir_all(&version_dir)
                .with_context(|| format!("Failed to remove version directory: {:?}", version_dir))?;
        }

        Ok(())
    }
}

impl Default for VersionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        let v = PhpVersion::parse("8.3").unwrap();
        assert_eq!(v.major, 8);
        assert_eq!(v.minor, 3);
        assert_eq!(v.patch, None);

        let v = PhpVersion::parse("8.3.15").unwrap();
        assert_eq!(v.major, 8);
        assert_eq!(v.minor, 3);
        assert_eq!(v.patch, Some(15));

        let v = PhpVersion::parse("^8.2").unwrap();
        assert_eq!(v.major, 8);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, None);
    }

    #[test]
    fn test_version_id() {
        let v = PhpVersion::parse("8.3.15").unwrap();
        assert_eq!(v.version_id(), 80315);

        let v = PhpVersion::parse("8.2").unwrap();
        assert_eq!(v.version_id(), 80200);
    }

    #[test]
    fn test_version_display() {
        let v = PhpVersion::parse("8.3.15").unwrap();
        assert_eq!(v.to_string(), "8.3.15");
        assert_eq!(v.to_string_short(), "8.3");

        let v = PhpVersion::parse("8.2").unwrap();
        assert_eq!(v.to_string(), "8.2");
        assert_eq!(v.to_string_short(), "8.2");
    }

    #[test]
    fn test_get_platform() {
        let platform = get_platform();
        assert!(!platform.is_empty());
        // Should contain architecture
        assert!(
            platform.contains("x86_64")
                || platform.contains("aarch64")
                || platform.contains("arm")
        );
    }
}
