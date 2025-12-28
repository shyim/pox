use serde::{Deserialize, Serialize};
use indexmap::IndexMap;

/// Autoload configuration for a Composer package
///
/// Defines how PHP classes should be autoloaded, supporting PSR-4, PSR-0,
/// classmap, and file-based autoloading strategies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Autoload {
    /// PSR-4 autoload mapping
    ///
    /// Maps namespace prefixes to directories. Each namespace can map to
    /// multiple directories.
    ///
    /// Example: `{"App\\": ["src/", "lib/"]}`
    #[serde(rename = "psr-4", skip_serializing_if = "IndexMap::is_empty", default)]
    pub psr4: IndexMap<String, AutoloadPath>,

    /// PSR-0 autoload mapping (legacy)
    ///
    /// Maps namespace prefixes to directories using the older PSR-0 standard.
    ///
    /// Example: `{"Monolog\\": ["src/", "lib/"]}`
    #[serde(rename = "psr-0", skip_serializing_if = "IndexMap::is_empty", default)]
    pub psr0: IndexMap<String, AutoloadPath>,

    /// Classmap autoload
    ///
    /// List of files/directories to scan for classes to build a classmap.
    ///
    /// Example: `["src/", "lib/", "Something.php"]`
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub classmap: Vec<String>,

    /// Files to always include
    ///
    /// List of files to be included on every request.
    ///
    /// Example: `["src/helpers.php", "src/constants.php"]`
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub files: Vec<String>,

    /// Paths to exclude from classmap generation
    ///
    /// Example: `["/Tests/", "/test/", "/tests/"]`
    #[serde(
        rename = "exclude-from-classmap",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub exclude_from_classmap: Vec<String>,
}

/// Represents an autoload path which can be either a single string or an array of strings
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoloadPath {
    /// Single directory path
    Single(String),
    /// Multiple directory paths
    Multiple(Vec<String>),
}

impl AutoloadPath {
    /// Returns the paths as a vector
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            AutoloadPath::Single(path) => vec![path.clone()],
            AutoloadPath::Multiple(paths) => paths.clone(),
        }
    }

    /// Returns an iterator over the paths
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        match self {
            AutoloadPath::Single(path) => vec![path].into_iter(),
            AutoloadPath::Multiple(paths) => paths.iter().collect::<Vec<_>>().into_iter(),
        }
    }
}

impl From<String> for AutoloadPath {
    fn from(path: String) -> Self {
        AutoloadPath::Single(path)
    }
}

impl From<&str> for AutoloadPath {
    fn from(path: &str) -> Self {
        AutoloadPath::Single(path.to_string())
    }
}

impl From<Vec<String>> for AutoloadPath {
    fn from(paths: Vec<String>) -> Self {
        if paths.len() == 1 {
            AutoloadPath::Single(paths[0].clone())
        } else {
            AutoloadPath::Multiple(paths)
        }
    }
}

impl Default for AutoloadPath {
    fn default() -> Self {
        AutoloadPath::Single(String::new())
    }
}

/// Convert from json::AutoloadPath to package::AutoloadPath
impl From<crate::json::AutoloadPath> for AutoloadPath {
    fn from(path: crate::json::AutoloadPath) -> Self {
        match path {
            crate::json::AutoloadPath::Single(s) => AutoloadPath::Single(s),
            crate::json::AutoloadPath::Multiple(v) => AutoloadPath::Multiple(v),
        }
    }
}

/// Convert from json::Autoload to package::Autoload
impl From<crate::json::Autoload> for Autoload {
    fn from(al: crate::json::Autoload) -> Self {
        Autoload {
            psr4: al.psr4.into_iter().map(|(k, v)| (k, v.into())).collect(),
            psr0: al.psr0.into_iter().map(|(k, v)| (k, v.into())).collect(),
            classmap: al.classmap,
            files: al.files,
            exclude_from_classmap: al.exclude_from_classmap,
        }
    }
}

impl Autoload {
    /// Creates a new empty autoload configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a PSR-4 namespace mapping
    pub fn add_psr4(mut self, namespace: impl Into<String>, path: impl Into<AutoloadPath>) -> Self {
        self.psr4.insert(namespace.into(), path.into());
        self
    }

    /// Adds a PSR-0 namespace mapping
    pub fn add_psr0(mut self, namespace: impl Into<String>, path: impl Into<AutoloadPath>) -> Self {
        self.psr0.insert(namespace.into(), path.into());
        self
    }

    /// Adds a classmap path
    pub fn add_classmap(mut self, path: impl Into<String>) -> Self {
        self.classmap.push(path.into());
        self
    }

    /// Adds a file to be included
    pub fn add_file(mut self, path: impl Into<String>) -> Self {
        self.files.push(path.into());
        self
    }

    /// Adds a path to exclude from classmap
    pub fn add_exclude(mut self, path: impl Into<String>) -> Self {
        self.exclude_from_classmap.push(path.into());
        self
    }

    /// Checks if the autoload configuration is empty
    pub fn is_empty(&self) -> bool {
        self.psr4.is_empty()
            && self.psr0.is_empty()
            && self.classmap.is_empty()
            && self.files.is_empty()
    }

    /// Merges another autoload configuration into this one
    pub fn merge(&mut self, other: Autoload) {
        self.psr4.extend(other.psr4);
        self.psr0.extend(other.psr0);
        self.classmap.extend(other.classmap);
        self.files.extend(other.files);
        self.exclude_from_classmap.extend(other.exclude_from_classmap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autoload_creation() {
        let autoload = Autoload::new()
            .add_psr4("App\\", "src/")
            .add_psr4("Tests\\", vec!["tests/".to_string()])
            .add_classmap("lib/")
            .add_file("src/helpers.php");

        assert_eq!(autoload.psr4.len(), 2);
        assert_eq!(autoload.classmap.len(), 1);
        assert_eq!(autoload.files.len(), 1);
    }

    #[test]
    fn test_autoload_is_empty() {
        let empty = Autoload::new();
        assert!(empty.is_empty());

        let not_empty = Autoload::new().add_psr4("App\\", "src/");
        assert!(!not_empty.is_empty());
    }

    #[test]
    fn test_autoload_merge() {
        let mut autoload1 = Autoload::new()
            .add_psr4("App\\", "src/")
            .add_classmap("lib/");

        let autoload2 = Autoload::new()
            .add_psr4("Tests\\", "tests/")
            .add_file("helpers.php");

        autoload1.merge(autoload2);

        assert_eq!(autoload1.psr4.len(), 2);
        assert_eq!(autoload1.classmap.len(), 1);
        assert_eq!(autoload1.files.len(), 1);
    }

    #[test]
    fn test_autoload_path_single() {
        let path = AutoloadPath::from("src/");
        assert_eq!(path.as_vec(), vec!["src/"]);
    }

    #[test]
    fn test_autoload_path_multiple() {
        let path = AutoloadPath::from(vec!["src/".to_string(), "lib/".to_string()]);
        assert_eq!(path.as_vec(), vec!["src/", "lib/"]);
    }

    #[test]
    fn test_autoload_serialization() {
        let autoload = Autoload::new()
            .add_psr4("App\\", "src/")
            .add_classmap("lib/");

        let json = serde_json::to_string(&autoload).unwrap();
        let deserialized: Autoload = serde_json::from_str(&json).unwrap();

        assert_eq!(autoload, deserialized);
    }
}
