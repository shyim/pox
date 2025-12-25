//! Artifact repository - discovers packages from archive files in a directory.

use std::fs::File;
use std::io::{Read, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use flate2::read::GzDecoder;
use sha1::{Sha1, Digest};
use tar::Archive as TarArchive;
use walkdir::WalkDir;
use zip::ZipArchive;

use super::traits::{Repository, SearchMode, SearchResult, ProviderInfo};
use crate::package::{Package, Dist, Autoload, AutoloadPath};

/// Artifact repository - provides packages from archive files in a directory
///
/// This repository type scans a directory for archive files (zip, tar, tar.gz, tgz)
/// and extracts package information from their composer.json files.
///
/// ```json
/// {
///     "repositories": [
///         {
///             "type": "artifact",
///             "url": "path/to/directory/with/zips/"
///         }
///     ]
/// }
/// ```
#[derive(Debug)]
pub struct ArtifactRepository {
    /// Repository name
    name: String,
    /// Directory path to scan for archives
    path: PathBuf,
    /// Discovered packages
    packages: Vec<Arc<Package>>,
}

impl ArtifactRepository {
    /// Create a new artifact repository
    ///
    /// # Arguments
    /// * `path` - Path to directory containing archive files
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = format!("artifact ({})", path.display());

        let mut repo = Self {
            name,
            path,
            packages: Vec::new(),
        };

        repo.scan_directory();
        repo
    }

    /// Scan the directory for archive files and extract package information
    fn scan_directory(&mut self) {
        if !self.path.exists() || !self.path.is_dir() {
            return;
        }

        for entry in WalkDir::new(&self.path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            // Check for supported archive extensions
            let ext = path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase());

            let archive_type = match ext.as_deref() {
                Some("zip") => ArchiveType::Zip,
                Some("tar") => ArchiveType::Tar,
                Some("gz") => {
                    // Check if it's .tar.gz
                    let stem = path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if stem.ends_with(".tar") {
                        ArchiveType::TarGz
                    } else {
                        continue; // Skip plain .gz files
                    }
                }
                Some("tgz") => ArchiveType::TarGz,
                _ => continue,
            };

            if let Some(package) = self.load_package_from_archive(path, archive_type) {
                self.packages.push(Arc::new(package));
            }
        }
    }

    /// Load package information from an archive file
    fn load_package_from_archive(&self, path: &Path, archive_type: ArchiveType) -> Option<Package> {
        let composer_json = match archive_type {
            ArchiveType::Zip => extract_composer_json_from_zip(path),
            ArchiveType::Tar | ArchiveType::TarGz => extract_composer_json_from_tar(path, archive_type == ArchiveType::TarGz),
        };

        let json: serde_json::Value = match composer_json {
            Some(content) => serde_json::from_str(&content).ok()?,
            None => return None,
        };

        // Required fields
        let name = json.get("name")?.as_str()?;
        let version = json.get("version")?.as_str()?;

        let mut pkg = Package::new(name, version);

        let shasum = calculate_sha1(path).ok();

        let dist_type = match archive_type {
            ArchiveType::Zip => "zip",
            ArchiveType::Tar => "tar",
            ArchiveType::TarGz => "tar",
        };

        let mut dist = Dist::new(dist_type, path.to_string_lossy().as_ref());
        if let Some(sha) = shasum {
            dist = dist.with_shasum(&sha);
        }
        pkg.dist = Some(dist);

        if let Some(desc) = json.get("description").and_then(|v| v.as_str()) {
            pkg.description = Some(desc.to_string());
        }

        if let Some(t) = json.get("type").and_then(|v| v.as_str()) {
            pkg.package_type = t.to_string();
        }

        if let Some(license) = json.get("license") {
            pkg.license = parse_license(license);
        }

        if let Some(require) = json.get("require").and_then(|v| v.as_object()) {
            pkg.require = require
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("*").to_string()))
                .collect();
        }

        if let Some(require_dev) = json.get("require-dev").and_then(|v| v.as_object()) {
            pkg.require_dev = require_dev
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("*").to_string()))
                .collect();
        }

        if let Some(autoload) = json.get("autoload") {
            pkg.autoload = Some(parse_autoload(autoload));
        }

        if let Some(autoload_dev) = json.get("autoload-dev") {
            pkg.autoload_dev = Some(parse_autoload(autoload_dev));
        }

        if let Some(bin) = json.get("bin").and_then(|v| v.as_array()) {
            pkg.bin = bin
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }

        // Replace self.version constraints with actual version
        pkg.replace_self_version();

        Some(pkg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveType {
    Zip,
    Tar,
    TarGz,
}

/// Extract composer.json content from a ZIP archive
fn extract_composer_json_from_zip(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut archive = ZipArchive::new(BufReader::new(file)).ok()?;

    // First, try to find composer.json at the root
    if let Ok(mut file) = archive.by_name("composer.json") {
        let mut content = String::new();
        file.read_to_string(&mut content).ok()?;
        return Some(content);
    }

    // If not at root, find the top-level directory and look there
    let mut top_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            let name = file.name();
            // Skip __MACOSX and other hidden directories
            if name.starts_with("__MACOSX") || name.starts_with('.') {
                continue;
            }

            // Get the first path component
            if let Some(first_component) = name.split('/').next() {
                if !first_component.is_empty() && name.contains('/') {
                    top_dirs.insert(first_component.to_string());
                }
            }
        }
    }

    // If there's exactly one top-level directory, look for composer.json there
    if top_dirs.len() == 1 {
        let top_dir = top_dirs.into_iter().next()?;
        let composer_path = format!("{}/composer.json", top_dir);

        if let Ok(mut file) = archive.by_name(&composer_path) {
            let mut content = String::new();
            file.read_to_string(&mut content).ok()?;
            return Some(content);
        }
    }

    None
}

/// Extract composer.json content from a TAR archive (optionally gzipped)
fn extract_composer_json_from_tar(path: &Path, gzipped: bool) -> Option<String> {
    let file = File::open(path).ok()?;

    if gzipped {
        let decoder = GzDecoder::new(BufReader::new(file));
        extract_composer_json_from_tar_reader(decoder)
    } else {
        extract_composer_json_from_tar_reader(BufReader::new(file))
    }
}

fn extract_composer_json_from_tar_reader<R: Read>(reader: R) -> Option<String> {
    let mut archive = TarArchive::new(reader);

    // Collect all entries to find composer.json
    let mut entries_info: Vec<(String, Vec<u8>)> = Vec::new();
    let mut top_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in archive.entries().ok()? {
        let mut entry = entry.ok()?;
        let path = entry.path().ok()?.to_string_lossy().to_string();

        // Skip hidden files
        if path.starts_with('.') || path.contains("/.") {
            continue;
        }

        // Track top-level directories
        if let Some(first_component) = path.split('/').next() {
            if !first_component.is_empty() && path.contains('/') {
                top_dirs.insert(first_component.to_string());
            }
        }

        // Check if this is composer.json at root
        if path == "composer.json" {
            let mut content = String::new();
            entry.read_to_string(&mut content).ok()?;
            return Some(content);
        }

        // Store entry data for later if it might be composer.json in a subdirectory
        if path.ends_with("/composer.json") || path.ends_with("composer.json") {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).ok()?;
            entries_info.push((path, data));
        }
    }

    // If there's exactly one top-level directory, look for composer.json there
    if top_dirs.len() == 1 {
        let top_dir = top_dirs.into_iter().next()?;
        let composer_path = format!("{}/composer.json", top_dir);

        for (path, data) in entries_info {
            if path == composer_path {
                return String::from_utf8(data).ok();
            }
        }
    }

    None
}

/// Calculate SHA1 hash of a file
fn calculate_sha1(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Parse license from JSON value
fn parse_license(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse autoload from JSON value
fn parse_autoload(value: &serde_json::Value) -> Autoload {
    let mut autoload = Autoload::default();

    if let Some(psr4) = value.get("psr-4").and_then(|v| v.as_object()) {
        for (namespace, paths) in psr4 {
            let path = json_to_autoload_path(paths);
            autoload.psr4.insert(namespace.clone(), path);
        }
    }

    if let Some(psr0) = value.get("psr-0").and_then(|v| v.as_object()) {
        for (namespace, paths) in psr0 {
            let path = json_to_autoload_path(paths);
            autoload.psr0.insert(namespace.clone(), path);
        }
    }

    if let Some(classmap) = value.get("classmap").and_then(|v| v.as_array()) {
        autoload.classmap = classmap
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }

    if let Some(files) = value.get("files").and_then(|v| v.as_array()) {
        autoload.files = files
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }

    autoload
}

/// Convert JSON value to AutoloadPath
fn json_to_autoload_path(value: &serde_json::Value) -> AutoloadPath {
    match value {
        serde_json::Value::String(s) => AutoloadPath::Single(s.clone()),
        serde_json::Value::Array(arr) => {
            let paths: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if paths.len() == 1 {
                AutoloadPath::Single(paths[0].clone())
            } else {
                AutoloadPath::Multiple(paths)
            }
        }
        _ => AutoloadPath::Single(String::new()),
    }
}

#[async_trait]
impl Repository for ArtifactRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn has_package(&self, name: &str) -> bool {
        self.packages.iter().any(|p| p.name.eq_ignore_ascii_case(name))
    }

    async fn find_packages(&self, name: &str) -> Vec<Arc<Package>> {
        self.packages
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .cloned()
            .collect()
    }

    async fn find_package(&self, name: &str, version: &str) -> Option<Arc<Package>> {
        self.packages
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name) && p.version == version)
            .cloned()
    }

    async fn find_packages_with_constraint(
        &self,
        name: &str,
        _constraint: &str,
    ) -> Vec<Arc<Package>> {
        // Return all versions, let the solver filter
        self.find_packages(name).await
    }

    async fn get_packages(&self) -> Vec<Arc<Package>> {
        self.packages.clone()
    }

    async fn search(&self, query: &str, _mode: SearchMode) -> Vec<SearchResult> {
        self.packages
            .iter()
            .filter(|p| {
                p.name.contains(query) ||
                p.description.as_ref().map(|d| d.contains(query)).unwrap_or(false)
            })
            .map(|p| SearchResult {
                name: p.name.clone(),
                description: p.description.clone(),
                url: None,
                abandoned: None,
            })
            .collect()
    }

    async fn get_providers(&self, _package_name: &str) -> Vec<ProviderInfo> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_zip(dir: &Path, name: &str, pkg_name: &str, version: &str) -> PathBuf {
        let zip_path = dir.join(name);
        let file = File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        let composer_json = serde_json::json!({
            "name": pkg_name,
            "version": version,
            "description": "Test package"
        });

        zip.start_file("composer.json", options).unwrap();
        zip.write_all(composer_json.to_string().as_bytes()).unwrap();

        zip.finish().unwrap();
        zip_path
    }

    fn create_test_zip_with_subdir(dir: &Path, name: &str, pkg_name: &str, version: &str) -> PathBuf {
        let zip_path = dir.join(name);
        let file = File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        let composer_json = serde_json::json!({
            "name": pkg_name,
            "version": version,
            "description": "Test package in subdirectory"
        });

        // Add directory entry
        zip.add_directory("package/", options).unwrap();

        // Add composer.json in subdirectory
        zip.start_file("package/composer.json", options).unwrap();
        zip.write_all(composer_json.to_string().as_bytes()).unwrap();

        zip.finish().unwrap();
        zip_path
    }

    #[tokio::test]
    async fn test_artifact_repository_zip() {
        let temp = TempDir::new().unwrap();
        create_test_zip(temp.path(), "package-1.0.0.zip", "vendor/package", "1.0.0");

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "vendor/package");
        assert_eq!(packages[0].version, "1.0.0");
        assert!(packages[0].dist.is_some());
    }

    #[tokio::test]
    async fn test_artifact_repository_zip_with_subdir() {
        let temp = TempDir::new().unwrap();
        create_test_zip_with_subdir(temp.path(), "package-1.0.0.zip", "vendor/package", "1.0.0");

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "vendor/package");
    }

    #[tokio::test]
    async fn test_artifact_repository_multiple_packages() {
        let temp = TempDir::new().unwrap();
        create_test_zip(temp.path(), "package-a-1.0.0.zip", "vendor/package-a", "1.0.0");
        create_test_zip(temp.path(), "package-b-2.0.0.zip", "vendor/package-b", "2.0.0");

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 2);
    }

    #[tokio::test]
    async fn test_artifact_repository_multiple_versions() {
        let temp = TempDir::new().unwrap();
        create_test_zip(temp.path(), "package-1.0.0.zip", "vendor/package", "1.0.0");
        create_test_zip(temp.path(), "package-2.0.0.zip", "vendor/package", "2.0.0");

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.find_packages("vendor/package").await;

        assert_eq!(packages.len(), 2);
    }

    #[tokio::test]
    async fn test_artifact_repository_find_package() {
        let temp = TempDir::new().unwrap();
        create_test_zip(temp.path(), "package-1.0.0.zip", "vendor/package", "1.0.0");
        create_test_zip(temp.path(), "package-2.0.0.zip", "vendor/package", "2.0.0");

        let repo = ArtifactRepository::new(temp.path());

        let found = repo.find_package("vendor/package", "1.0.0").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().version, "1.0.0");

        let not_found = repo.find_package("vendor/package", "3.0.0").await;
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_artifact_repository_with_metadata() {
        let temp = TempDir::new().unwrap();
        let zip_path = temp.path().join("package.zip");
        let file = File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);

        let options = zip::write::SimpleFileOptions::default();

        let composer_json = serde_json::json!({
            "name": "vendor/package",
            "version": "1.0.0",
            "description": "A test package",
            "type": "library",
            "license": "MIT",
            "require": {
                "php": ">=8.0"
            },
            "autoload": {
                "psr-4": {
                    "Vendor\\Package\\": "src/"
                }
            }
        });

        zip.start_file("composer.json", options).unwrap();
        zip.write_all(composer_json.to_string().as_bytes()).unwrap();
        zip.finish().unwrap();

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 1);
        let pkg = &packages[0];
        assert_eq!(pkg.description, Some("A test package".to_string()));
        assert_eq!(pkg.package_type, "library");
        assert_eq!(pkg.license, vec!["MIT".to_string()]);
        assert!(pkg.require.contains_key("php"));
        assert!(pkg.autoload.is_some());
    }

    #[tokio::test]
    async fn test_artifact_repository_skips_invalid() {
        let temp = TempDir::new().unwrap();

        // Create a valid package
        create_test_zip(temp.path(), "valid.zip", "vendor/valid", "1.0.0");

        // Create an invalid zip (no composer.json)
        let invalid_path = temp.path().join("invalid.zip");
        let file = File::create(&invalid_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("readme.txt", options).unwrap();
        zip.write_all(b"No composer.json here").unwrap();
        zip.finish().unwrap();

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.get_packages().await;

        // Should only have the valid package
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "vendor/valid");
    }

    #[tokio::test]
    async fn test_artifact_repository_sha1_checksum() {
        let temp = TempDir::new().unwrap();
        create_test_zip(temp.path(), "package.zip", "vendor/package", "1.0.0");

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.get_packages().await;

        assert_eq!(packages.len(), 1);
        let dist = packages[0].dist.as_ref().unwrap();
        assert!(dist.shasum.is_some());
        // SHA1 is 40 hex characters
        assert_eq!(dist.shasum.as_ref().unwrap().len(), 40);
    }

    #[tokio::test]
    async fn test_artifact_repository_empty_directory() {
        let temp = TempDir::new().unwrap();

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.get_packages().await;

        assert!(packages.is_empty());
    }

    #[tokio::test]
    async fn test_artifact_repository_nonexistent_directory() {
        let repo = ArtifactRepository::new("/nonexistent/path");
        let packages = repo.get_packages().await;

        assert!(packages.is_empty());
    }

    #[tokio::test]
    async fn test_artifact_repository_subdirectories() {
        let temp = TempDir::new().unwrap();

        // Create a subdirectory with packages
        let subdir = temp.path().join("packages");
        std::fs::create_dir(&subdir).unwrap();
        create_test_zip(&subdir, "package.zip", "vendor/package", "1.0.0");

        let repo = ArtifactRepository::new(temp.path());
        let packages = repo.get_packages().await;

        // Should find packages in subdirectories
        assert_eq!(packages.len(), 1);
    }

    #[tokio::test]
    async fn test_search() {
        let temp = TempDir::new().unwrap();
        create_test_zip(temp.path(), "foo.zip", "vendor/foo-package", "1.0.0");
        create_test_zip(temp.path(), "bar.zip", "vendor/bar-package", "1.0.0");

        let repo = ArtifactRepository::new(temp.path());

        let results = repo.search("foo", SearchMode::Name).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "vendor/foo-package");
    }
}
