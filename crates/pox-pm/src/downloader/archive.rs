//! Archive extraction (zip, tar, tar.gz, tar.bz2).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use flate2::read::GzDecoder;

use crate::{ComposerError, Result};

/// Supported archive types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveType {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
}

impl ArchiveType {
    /// Detect archive type from file extension
    pub fn from_path(path: &Path) -> Option<Self> {
        let path_str = path.to_string_lossy().to_lowercase();

        if path_str.ends_with(".zip") {
            Some(ArchiveType::Zip)
        } else if path_str.ends_with(".tar.gz") || path_str.ends_with(".tgz") {
            Some(ArchiveType::TarGz)
        } else if path_str.ends_with(".tar.bz2") || path_str.ends_with(".tbz2") {
            Some(ArchiveType::TarBz2)
        } else if path_str.ends_with(".tar.xz") || path_str.ends_with(".txz") {
            Some(ArchiveType::TarXz)
        } else if path_str.ends_with(".tar") {
            Some(ArchiveType::Tar)
        } else {
            None
        }
    }

    /// Detect archive type from content type header
    pub fn from_content_type(content_type: &str) -> Option<Self> {
        let ct = content_type.to_lowercase();

        // Check more specific types first
        if ct.contains("gzip") || ct.contains("x-gzip") {
            Some(ArchiveType::TarGz)
        } else if ct.contains("bzip2") || ct.contains("x-bzip2") {
            Some(ArchiveType::TarBz2)
        } else if ct.contains("x-xz") {
            Some(ArchiveType::TarXz)
        } else if ct.contains("x-tar") {
            Some(ArchiveType::Tar)
        } else if ct.contains("zip") {
            Some(ArchiveType::Zip)
        } else {
            None
        }
    }
}

/// Archive extractor
pub struct ArchiveExtractor;

impl ArchiveExtractor {
    /// Extract an archive to the specified directory
    pub fn extract(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        let archive_type = ArchiveType::from_path(archive_path)
            .ok_or_else(|| ComposerError::InstallationFailed(
                format!("Unknown archive type: {}", archive_path.display())
            ))?;

        Self::extract_with_type(archive_path, dest_dir, archive_type)
    }

    /// Extract an archive with explicit type
    pub fn extract_with_type(
        archive_path: &Path,
        dest_dir: &Path,
        archive_type: ArchiveType,
    ) -> Result<()> {
        // Create destination directory
        std::fs::create_dir_all(dest_dir)?;

        match archive_type {
            ArchiveType::Zip => Self::extract_zip(archive_path, dest_dir),
            ArchiveType::Tar => Self::extract_tar(archive_path, dest_dir),
            ArchiveType::TarGz => Self::extract_tar_gz(archive_path, dest_dir),
            ArchiveType::TarBz2 => Self::extract_tar_bz2(archive_path, dest_dir),
            ArchiveType::TarXz => Self::extract_tar_xz(archive_path, dest_dir),
        }
    }

    /// Extract a zip archive
    fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        let file = File::open(archive_path)?;
        let reader = BufReader::new(file);
        let mut archive = zip::ZipArchive::new(reader)
            .map_err(|e| ComposerError::InstallationFailed(format!("Failed to open zip: {}", e)))?;

        // Find common prefix (GitHub archives have vendor-package-hash/ prefix)
        let common_prefix = Self::find_zip_common_prefix(&archive);

        // Canonicalize dest_dir for path traversal check
        let dest_dir_canonical = dest_dir.canonicalize()
            .map_err(|e| ComposerError::InstallationFailed(format!("Failed to canonicalize destination: {}", e)))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)
                .map_err(|e| ComposerError::InstallationFailed(format!("Failed to read zip entry: {}", e)))?;

            let mut outpath = dest_dir.to_path_buf();

            // Get the file path and strip common prefix if present
            let name = file.name();
            let relative_path = if let Some(ref prefix) = common_prefix {
                name.strip_prefix(prefix).unwrap_or(name)
            } else {
                name
            };

            // Skip empty paths
            if relative_path.is_empty() {
                continue;
            }

            // Validate path doesn't contain traversal sequences
            if relative_path.contains("..") {
                return Err(ComposerError::InstallationFailed(
                    format!("Path traversal detected in archive: {}", relative_path)
                ));
            }

            outpath.push(relative_path);

            // Verify the path stays within destination directory
            // Create parent dirs first so we can canonicalize
            if file.is_dir() {
                std::fs::create_dir_all(&outpath)?;
            } else if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // After creating directories, verify the path is safe
            let outpath_canonical = outpath.canonicalize().unwrap_or_else(|_| {
                // For new files, canonicalize the parent and append filename
                if let Some(parent) = outpath.parent() {
                    if let Ok(parent_canonical) = parent.canonicalize() {
                        if let Some(filename) = outpath.file_name() {
                            return parent_canonical.join(filename);
                        }
                    }
                }
                outpath.clone()
            });

            if !outpath_canonical.starts_with(&dest_dir_canonical) {
                return Err(ComposerError::InstallationFailed(
                    format!("Path traversal detected: {} escapes destination directory", relative_path)
                ));
            }

            if file.is_dir() {
                // Already created above for canonicalization
            } else {
                let mut outfile = File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;

                // Set permissions on Unix
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Some(mode) = file.unix_mode() {
                        std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode))?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Find common prefix in zip archive (e.g., vendor-package-hash/)
    fn find_zip_common_prefix(archive: &zip::ZipArchive<BufReader<File>>) -> Option<String> {
        if archive.is_empty() {
            return None;
        }

        // Get first entry's name
        let first_name = archive.name_for_index(0)?;

        // Find the first directory component
        let prefix = if let Some(slash_pos) = first_name.find('/') {
            &first_name[..=slash_pos]
        } else {
            return None;
        };

        // Check if all entries share this prefix
        for i in 0..archive.len() {
            if let Some(name) = archive.name_for_index(i) {
                if !name.starts_with(prefix) {
                    return None;
                }
            }
        }

        Some(prefix.to_string())
    }

    /// Extract a plain tar archive
    fn extract_tar(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        let file = File::open(archive_path)?;
        let reader = BufReader::new(file);
        Self::extract_tar_reader(reader, dest_dir)
    }

    /// Extract a gzipped tar archive
    fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        let file = File::open(archive_path)?;
        let reader = BufReader::new(file);
        let decoder = GzDecoder::new(reader);
        Self::extract_tar_reader(decoder, dest_dir)
    }

    /// Extract a bzip2 tar archive
    fn extract_tar_bz2(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        use bzip2::read::BzDecoder;

        let file = File::open(archive_path)?;
        let reader = BufReader::new(file);
        let decoder = BzDecoder::new(reader);
        Self::extract_tar_reader(decoder, dest_dir)
    }

    /// Extract an xz tar archive
    fn extract_tar_xz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
        use xz2::read::XzDecoder;

        let file = File::open(archive_path)?;
        let reader = BufReader::new(file);
        let decoder = XzDecoder::new(reader);
        Self::extract_tar_reader(decoder, dest_dir)
    }

    /// Extract from a tar reader (common implementation)
    /// Strips the first component (GitHub-style vendor-package-ref/ prefix)
    fn extract_tar_reader<R: Read>(reader: R, dest_dir: &Path) -> Result<()> {
        Self::extract_tar_with_strip(reader, dest_dir, 1)
    }

    /// Extract tar with prefix stripping
    pub fn extract_tar_with_strip<R: Read>(reader: R, dest_dir: &Path, strip_components: usize) -> Result<()> {
        let mut archive = tar::Archive::new(reader);

        // Canonicalize dest_dir for path traversal check
        let dest_dir_canonical = dest_dir.canonicalize()
            .map_err(|e| ComposerError::InstallationFailed(format!("Failed to canonicalize destination: {}", e)))?;

        for entry in archive.entries()
            .map_err(|e| ComposerError::InstallationFailed(format!("Failed to read tar: {}", e)))?
        {
            let mut entry = entry
                .map_err(|e| ComposerError::InstallationFailed(format!("Failed to read tar entry: {}", e)))?;

            let path = entry.path()
                .map_err(|e| ComposerError::InstallationFailed(format!("Invalid path in tar: {}", e)))?;

            // Strip leading components
            let components: Vec<_> = path.components().collect();
            if components.len() <= strip_components {
                continue;
            }

            let stripped: std::path::PathBuf = components[strip_components..].iter().collect();
            if stripped.as_os_str().is_empty() {
                continue;
            }

            // Validate path doesn't contain traversal sequences
            let stripped_str = stripped.to_string_lossy();
            if stripped_str.contains("..") {
                return Err(ComposerError::InstallationFailed(
                    format!("Path traversal detected in archive: {}", stripped_str)
                ));
            }

            let outpath = dest_dir.join(&stripped);

            // Create parent directories first so we can verify the path
            if entry.header().entry_type().is_dir() {
                std::fs::create_dir_all(&outpath)?;
            } else if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Verify the path stays within destination directory
            let outpath_canonical = outpath.canonicalize().unwrap_or_else(|_| {
                // For new files, canonicalize the parent and append filename
                if let Some(parent) = outpath.parent() {
                    if let Ok(parent_canonical) = parent.canonicalize() {
                        if let Some(filename) = outpath.file_name() {
                            return parent_canonical.join(filename);
                        }
                    }
                }
                outpath.clone()
            });

            if !outpath_canonical.starts_with(&dest_dir_canonical) {
                return Err(ComposerError::InstallationFailed(
                    format!("Path traversal detected: {} escapes destination directory", stripped_str)
                ));
            }

            if entry.header().entry_type().is_dir() {
                // Already created above
            } else {
                entry.unpack(&outpath)
                    .map_err(|e| ComposerError::InstallationFailed(format!("Failed to extract: {}", e)))?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_type_from_path() {
        assert_eq!(
            ArchiveType::from_path(Path::new("package.zip")),
            Some(ArchiveType::Zip)
        );
        assert_eq!(
            ArchiveType::from_path(Path::new("package.tar.gz")),
            Some(ArchiveType::TarGz)
        );
        assert_eq!(
            ArchiveType::from_path(Path::new("package.tgz")),
            Some(ArchiveType::TarGz)
        );
        assert_eq!(
            ArchiveType::from_path(Path::new("package.tar.bz2")),
            Some(ArchiveType::TarBz2)
        );
        assert_eq!(
            ArchiveType::from_path(Path::new("package.tar")),
            Some(ArchiveType::Tar)
        );
        assert_eq!(
            ArchiveType::from_path(Path::new("package.txt")),
            None
        );
    }

    #[test]
    fn test_archive_type_from_content_type() {
        assert_eq!(
            ArchiveType::from_content_type("application/zip"),
            Some(ArchiveType::Zip)
        );
        assert_eq!(
            ArchiveType::from_content_type("application/gzip"),
            Some(ArchiveType::TarGz)
        );
        assert_eq!(
            ArchiveType::from_content_type("application/x-tar"),
            Some(ArchiveType::Tar)
        );
    }
}
