use std::collections::HashMap;
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};

/// Deserializes a HashMap that might be represented as an empty array in JSON.
/// Composer outputs `[]` for empty maps like stability-flags, platform-dev, etc.
fn deserialize_map_or_empty_array<'de, D, K, V>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + std::hash::Hash + Eq,
    V: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    #[serde(bound(deserialize = "K: Deserialize<'de> + std::hash::Hash + Eq, V: Deserialize<'de>"))]
    enum MapOrArray<K, V> {
        Map(HashMap<K, V>),
        #[allow(dead_code)]
        Array(Vec<serde_json::Value>),
    }

    match MapOrArray::deserialize(deserializer)? {
        MapOrArray::Map(map) => Ok(map),
        MapOrArray::Array(_) => Ok(HashMap::new()),
    }
}

/// Deserializes an IndexMap that might be represented as an empty array in JSON.
/// Composer outputs `[]` for empty maps like stability-flags, platform-dev, etc.
fn deserialize_indexmap_or_empty_array<'de, D, K, V>(deserializer: D) -> Result<IndexMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + std::hash::Hash + Eq,
    V: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    #[serde(bound(deserialize = "K: Deserialize<'de> + std::hash::Hash + Eq, V: Deserialize<'de>"))]
    enum MapOrArray<K, V> {
        Map(IndexMap<K, V>),
        #[allow(dead_code)]
        Array(Vec<serde_json::Value>),
    }

    match MapOrArray::deserialize(deserializer)? {
        MapOrArray::Map(map) => Ok(map),
        MapOrArray::Array(_) => Ok(IndexMap::new()),
    }
}

// Old implementation (kept for HashMap fields)
fn _old_deserialize_map_or_empty_array<'de, D, K, V>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + std::hash::Hash + Eq,
    V: Deserialize<'de>,
{
    use serde::de::{self, Visitor, MapAccess, SeqAccess};
    use std::marker::PhantomData;

    struct MapOrEmptyArrayVisitor<K, V> {
        marker: PhantomData<HashMap<K, V>>,
    }

    impl<'de, K, V> Visitor<'de> for MapOrEmptyArrayVisitor<K, V>
    where
        K: Deserialize<'de> + std::hash::Hash + Eq,
        V: Deserialize<'de>,
    {
        type Value = HashMap<K, V>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map or an empty array")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            // Only accept empty arrays
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(de::Error::custom("expected empty array or map, got non-empty array"));
            }
            Ok(HashMap::new())
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut result = HashMap::new();
            while let Some((key, value)) = map.next_entry()? {
                result.insert(key, value);
            }
            Ok(result)
        }
    }

    deserializer.deserialize_any(MapOrEmptyArrayVisitor {
        marker: PhantomData,
    })
}

/// Represents a composer.lock file
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ComposerLock {
    /// Readme note
    #[serde(default = "default_readme", rename = "_readme", skip_serializing_if = "Vec::is_empty")]
    pub readme: Vec<String>,

    /// Content hash for detecting changes
    #[serde(default)]
    pub content_hash: String,

    /// Production packages
    #[serde(default)]
    pub packages: Vec<LockedPackage>,

    /// Development packages
    #[serde(default, rename = "packages-dev")]
    pub packages_dev: Vec<LockedPackage>,

    /// Package aliases
    #[serde(default)]
    pub aliases: Vec<LockAlias>,

    /// Minimum stability
    #[serde(default)]
    pub minimum_stability: String,

    /// Per-package stability flags
    #[serde(default, deserialize_with = "deserialize_map_or_empty_array")]
    pub stability_flags: HashMap<String, u8>,

    /// Whether to prefer stable versions
    #[serde(default)]
    pub prefer_stable: bool,

    /// Whether to prefer lowest versions
    #[serde(default)]
    pub prefer_lowest: bool,

    /// Platform requirements
    #[serde(default, skip_serializing_if = "HashMap::is_empty", deserialize_with = "deserialize_map_or_empty_array")]
    pub platform: HashMap<String, String>,

    /// Platform dev requirements
    #[serde(default, skip_serializing_if = "HashMap::is_empty", deserialize_with = "deserialize_map_or_empty_array")]
    pub platform_dev: HashMap<String, String>,

    /// Platform overrides from config
    #[serde(default, skip_serializing_if = "HashMap::is_empty", deserialize_with = "deserialize_map_or_empty_array")]
    pub platform_overrides: HashMap<String, String>,

    /// Plugin API version used to generate this lock file
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plugin_api_version: String,
}

fn default_readme() -> Vec<String> {
    vec![
        "This file locks the dependencies of your project to a known state".to_string(),
        "Read more about it at https://getcomposer.org/doc/01-basic-usage.md#installing-dependencies".to_string(),
        "This file is @generated automatically".to_string(),
    ]
}

impl Default for ComposerLock {
    fn default() -> Self {
        Self {
            readme: default_readme(),
            content_hash: String::new(),
            packages: Vec::new(),
            packages_dev: Vec::new(),
            aliases: Vec::new(),
            minimum_stability: String::new(),
            stability_flags: HashMap::new(),
            prefer_stable: false,
            prefer_lowest: false,
            platform: HashMap::new(),
            platform_dev: HashMap::new(),
            platform_overrides: HashMap::new(),
            plugin_api_version: String::new(),
        }
    }
}

/// A locked package entry
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct LockedPackage {
    /// Package name (vendor/package)
    pub name: String,

    /// Version string
    pub version: String,

    /// Source information (VCS)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<LockSource>,

    /// Distribution information (archive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist: Option<LockDist>,

    /// Required packages
    #[serde(default, skip_serializing_if = "IndexMap::is_empty", deserialize_with = "deserialize_indexmap_or_empty_array")]
    pub require: IndexMap<String, String>,

    /// Development requirements
    #[serde(default, rename = "require-dev", skip_serializing_if = "IndexMap::is_empty", deserialize_with = "deserialize_indexmap_or_empty_array")]
    pub require_dev: IndexMap<String, String>,

    /// Conflicts
    #[serde(default, skip_serializing_if = "IndexMap::is_empty", deserialize_with = "deserialize_indexmap_or_empty_array")]
    pub conflict: IndexMap<String, String>,

    /// Provided packages
    #[serde(default, skip_serializing_if = "IndexMap::is_empty", deserialize_with = "deserialize_indexmap_or_empty_array")]
    pub provide: IndexMap<String, String>,

    /// Replaced packages
    #[serde(default, skip_serializing_if = "IndexMap::is_empty", deserialize_with = "deserialize_indexmap_or_empty_array")]
    pub replace: IndexMap<String, String>,

    /// Suggested packages
    #[serde(default, skip_serializing_if = "IndexMap::is_empty", deserialize_with = "deserialize_indexmap_or_empty_array")]
    pub suggest: IndexMap<String, String>,

    /// Binary executables
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bin: Vec<String>,

    /// Package type
    #[serde(rename = "type", default = "default_type")]
    pub package_type: String,

    /// Extra metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,

    /// Autoload configuration
    #[serde(default, skip_serializing_if = "LockAutoload::is_empty")]
    pub autoload: LockAutoload,

    /// Dev autoload configuration
    #[serde(default, rename = "autoload-dev", skip_serializing_if = "LockAutoload::is_empty")]
    pub autoload_dev: LockAutoload,

    /// Packagist notification URL
    #[serde(default, rename = "notification-url", skip_serializing_if = "Option::is_none")]
    pub notification_url: Option<String>,

    /// License(s)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub license: Vec<String>,

    /// Authors
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<LockAuthor>,

    /// Package description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Homepage URL
    #[serde(default, skip_serializing_if = "is_none_or_empty")]
    pub homepage: Option<String>,

    /// Keywords for search
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,

    /// Support information
    #[serde(default, skip_serializing_if = "HashMap::is_empty", deserialize_with = "deserialize_map_or_empty_array")]
    pub support: HashMap<String, String>,

    /// Funding information
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub funding: Vec<LockFunding>,

    /// Release time
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,

    /// Whether this is an abandoned package
    #[serde(default, skip_serializing_if = "is_null_or_false")]
    pub abandoned: serde_json::Value,

    /// Archive exclusions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<LockArchive>,

    /// Installation source preference
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_source: Option<String>,

    /// Default branch flag
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<bool>,
}

fn is_null_or_false(v: &serde_json::Value) -> bool {
    v.is_null() || v == &serde_json::Value::Bool(false)
}

fn is_none_or_empty(v: &Option<String>) -> bool {
    match v {
        None => true,
        Some(s) => s.is_empty(),
    }
}

fn default_type() -> String {
    "library".to_string()
}

/// Source information for VCS-based packages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockSource {
    /// Source type (git, hg, svn)
    #[serde(rename = "type")]
    pub source_type: String,

    /// Repository URL
    pub url: String,

    /// Commit/tag/branch reference
    pub reference: String,
}

/// Distribution information for archive-based packages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockDist {
    /// Distribution type (zip, tar, etc.)
    #[serde(rename = "type")]
    pub dist_type: String,

    /// Download URL
    pub url: String,

    /// Reference (optional for dist)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,

    /// SHA sum for verification (empty string when not available)
    #[serde(default, serialize_with = "serialize_shasum")]
    pub shasum: Option<String>,
}

fn serialize_shasum<S>(shasum: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // Composer always outputs shasum field, empty string when not available
    serializer.serialize_str(shasum.as_deref().unwrap_or(""))
}

/// Autoload configuration in lock file
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct LockAutoload {
    /// PSR-4 autoloading
    #[serde(default, rename = "psr-4", skip_serializing_if = "HashMap::is_empty", deserialize_with = "deserialize_map_or_empty_array")]
    pub psr4: HashMap<String, serde_json::Value>,

    /// PSR-0 autoloading
    #[serde(default, rename = "psr-0", skip_serializing_if = "HashMap::is_empty", deserialize_with = "deserialize_map_or_empty_array")]
    pub psr0: HashMap<String, serde_json::Value>,

    /// Classmap files/directories
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classmap: Vec<String>,

    /// Files to always include
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,

    /// Paths to exclude from classmap
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_from_classmap: Vec<String>,
}

impl LockAutoload {
    pub fn is_empty(&self) -> bool {
        self.psr4.is_empty() && self.psr0.is_empty() && self.classmap.is_empty() && self.files.is_empty()
    }
}

/// Author information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockAuthor {
    /// Author name
    pub name: String,

    /// Author email
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// Author homepage
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,

    /// Author role
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Funding information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockFunding {
    /// Funding URL
    pub url: String,

    /// Funding type (github, patreon, custom, etc.)
    #[serde(rename = "type")]
    pub funding_type: String,
}

/// Archive configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockArchive {
    /// Name for the archive
    #[serde(default)]
    pub name: Option<String>,

    /// Paths to exclude from archives
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Package alias
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockAlias {
    /// Package name
    pub package: String,

    /// Original version
    pub version: String,

    /// Aliased version
    pub alias: String,

    /// Normalized aliased version
    pub alias_normalized: String,
}

impl ComposerLock {
    /// Parse a composer.lock from JSON string
    pub fn from_str(content: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(content)
    }

    /// Parse a composer.lock from a file path
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, LockLoadError> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| LockLoadError::Io(e))?;
        Self::from_str(&content)
            .map_err(|e| LockLoadError::Parse(e))
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Get all packages (both prod and dev)
    pub fn all_packages(&self) -> impl Iterator<Item = &LockedPackage> {
        self.packages.iter().chain(self.packages_dev.iter())
    }

    /// Find a package by name
    pub fn find_package(&self, name: &str) -> Option<&LockedPackage> {
        let name_lower = name.to_lowercase();
        self.all_packages()
            .find(|p| p.name.to_lowercase() == name_lower)
    }

    /// Check if a package is a dev dependency
    pub fn is_dev_package(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();
        self.packages_dev.iter()
            .any(|p| p.name.to_lowercase() == name_lower)
    }

    /// Get the total number of locked packages
    pub fn package_count(&self) -> usize {
        self.packages.len() + self.packages_dev.len()
    }
}

impl LockedPackage {
    /// Get the best download URL (prefer dist over source)
    pub fn download_url(&self) -> Option<&str> {
        self.dist.as_ref()
            .map(|d| d.url.as_str())
            .or_else(|| self.source.as_ref().map(|s| s.url.as_str()))
    }

    /// Get the reference (commit hash, tag, etc.)
    pub fn reference(&self) -> Option<&str> {
        self.dist.as_ref()
            .and_then(|d| d.reference.as_deref())
            .or_else(|| self.source.as_ref().map(|s| s.reference.as_str()))
    }

    /// Check if this is an abandoned package
    pub fn is_abandoned(&self) -> bool {
        match &self.abandoned {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::String(_) => true,
            _ => false,
        }
    }

    /// Get the replacement package name if abandoned
    pub fn abandoned_replacement(&self) -> Option<&str> {
        match &self.abandoned {
            serde_json::Value::String(s) if !s.is_empty() => Some(s.as_str()),
            _ => None,
        }
    }
}

/// Errors that can occur when loading a lock file
#[derive(Debug)]
pub enum LockLoadError {
    Io(std::io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for LockLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockLoadError::Io(e) => write!(f, "Failed to read lock file: {}", e),
            LockLoadError::Parse(e) => write!(f, "Failed to parse lock file: {}", e),
        }
    }
}

impl std::error::Error for LockLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LockLoadError::Io(e) => Some(e),
            LockLoadError::Parse(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_lock() {
        let json = r#"{
            "content-hash": "abc123",
            "packages": [],
            "packages-dev": []
        }"#;

        let lock = ComposerLock::from_str(json).unwrap();
        assert_eq!(lock.content_hash, "abc123");
        assert!(lock.packages.is_empty());
        assert!(lock.packages_dev.is_empty());
    }

    #[test]
    fn test_parse_package() {
        let json = r#"{
            "content-hash": "abc123",
            "packages": [{
                "name": "vendor/package",
                "version": "1.0.0",
                "source": {
                    "type": "git",
                    "url": "https://github.com/vendor/package.git",
                    "reference": "abc123def"
                },
                "dist": {
                    "type": "zip",
                    "url": "https://example.com/package.zip",
                    "shasum": "sha256hash"
                },
                "require": {
                    "php": ">=8.0"
                },
                "type": "library",
                "description": "A test package"
            }],
            "packages-dev": []
        }"#;

        let lock = ComposerLock::from_str(json).unwrap();
        assert_eq!(lock.packages.len(), 1);

        let pkg = &lock.packages[0];
        assert_eq!(pkg.name, "vendor/package");
        assert_eq!(pkg.version, "1.0.0");
        assert_eq!(pkg.package_type, "library");

        let source = pkg.source.as_ref().unwrap();
        assert_eq!(source.source_type, "git");
        assert_eq!(source.reference, "abc123def");

        let dist = pkg.dist.as_ref().unwrap();
        assert_eq!(dist.dist_type, "zip");
    }

    #[test]
    fn test_find_package() {
        let json = r#"{
            "content-hash": "abc",
            "packages": [{"name": "vendor/prod", "version": "1.0.0"}],
            "packages-dev": [{"name": "vendor/dev", "version": "2.0.0"}]
        }"#;

        let lock = ComposerLock::from_str(json).unwrap();

        assert!(lock.find_package("vendor/prod").is_some());
        assert!(lock.find_package("vendor/dev").is_some());
        assert!(lock.find_package("VENDOR/PROD").is_some()); // case-insensitive
        assert!(lock.find_package("nonexistent").is_none());

        assert!(!lock.is_dev_package("vendor/prod"));
        assert!(lock.is_dev_package("vendor/dev"));
    }

    #[test]
    fn test_abandoned_package() {
        let json = r#"{
            "content-hash": "abc",
            "packages": [
                {"name": "pkg1", "version": "1.0", "abandoned": false},
                {"name": "pkg2", "version": "1.0", "abandoned": true},
                {"name": "pkg3", "version": "1.0", "abandoned": "new/package"}
            ],
            "packages-dev": []
        }"#;

        let lock = ComposerLock::from_str(json).unwrap();

        assert!(!lock.packages[0].is_abandoned());
        assert!(lock.packages[1].is_abandoned());
        assert!(lock.packages[2].is_abandoned());

        assert!(lock.packages[0].abandoned_replacement().is_none());
        assert!(lock.packages[1].abandoned_replacement().is_none());
        assert_eq!(lock.packages[2].abandoned_replacement(), Some("new/package"));
    }

    #[test]
    fn test_parse_empty_arrays_as_maps() {
        // Composer outputs empty arrays [] instead of empty objects {} for some fields
        let json = r#"{
            "content-hash": "abc123",
            "packages": [],
            "packages-dev": [],
            "aliases": [],
            "minimum-stability": "stable",
            "stability-flags": [],
            "prefer-stable": true,
            "prefer-lowest": false,
            "platform": {
                "php": ">=8.2"
            },
            "platform-dev": [],
            "plugin-api-version": "2.9.0"
        }"#;

        let lock = ComposerLock::from_str(json).unwrap();
        assert!(lock.stability_flags.is_empty());
        assert!(lock.platform_dev.is_empty());
        assert_eq!(lock.platform.get("php"), Some(&">=8.2".to_string()));
    }

    #[test]
    fn test_parse_package_with_empty_arrays() {
        let json = r#"{
            "content-hash": "abc123",
            "packages": [{
                "name": "vendor/package",
                "version": "1.0.0",
                "require": [],
                "require-dev": [],
                "conflict": [],
                "provide": [],
                "replace": [],
                "suggest": [],
                "type": "library"
            }],
            "packages-dev": []
        }"#;

        let lock = ComposerLock::from_str(json).unwrap();
        assert_eq!(lock.packages.len(), 1);

        let pkg = &lock.packages[0];
        assert!(pkg.require.is_empty());
        assert!(pkg.require_dev.is_empty());
        assert!(pkg.conflict.is_empty());
        assert!(pkg.provide.is_empty());
        assert!(pkg.replace.is_empty());
        assert!(pkg.suggest.is_empty());
    }
}
