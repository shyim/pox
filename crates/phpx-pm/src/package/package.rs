use super::{Autoload, Dist, Link, Source};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Package stability levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stability {
    /// Development version
    Dev,
    /// Alpha release
    Alpha,
    /// Beta release
    Beta,
    /// Release candidate
    #[serde(rename = "RC")]
    RC,
    /// Stable release
    Stable,
}

impl Stability {
    /// Returns the stability priority (lower is more stable)
    pub fn priority(&self) -> u8 {
        match self {
            Stability::Stable => 0,
            Stability::RC => 5,
            Stability::Beta => 10,
            Stability::Alpha => 15,
            Stability::Dev => 20,
        }
    }

    /// Parses stability from a version string
    pub fn from_version(version: &str) -> Self {
        let lower = version.to_lowercase();
        if lower.contains("dev") {
            Stability::Dev
        } else if lower.contains("alpha") {
            Stability::Alpha
        } else if lower.contains("beta") {
            Stability::Beta
        } else if lower.contains("rc") {
            Stability::RC
        } else {
            Stability::Stable
        }
    }

    /// Parse stability from a string (e.g., from composer.json minimum-stability)
    fn parse_stability(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "dev" => Stability::Dev,
            "alpha" => Stability::Alpha,
            "beta" => Stability::Beta,
            "rc" => Stability::RC,
            "stable" | "" => Stability::Stable,
            _ => Stability::Stable, // Default to stable for unknown values
        }
    }
}

impl Default for Stability {
    fn default() -> Self {
        Stability::Stable
    }
}

impl std::str::FromStr for Stability {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Stability::parse_stability(s))
    }
}

impl std::fmt::Display for Stability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stability::Dev => write!(f, "dev"),
            Stability::Alpha => write!(f, "alpha"),
            Stability::Beta => write!(f, "beta"),
            Stability::RC => write!(f, "RC"),
            Stability::Stable => write!(f, "stable"),
        }
    }
}

/// Information about abandoned packages
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Abandoned {
    /// Package is abandoned with no replacement
    Yes,
    /// Package is abandoned with a suggested replacement
    Replacement(String),
}

impl Abandoned {
    /// Returns true if the package is abandoned
    pub fn is_abandoned(&self) -> bool {
        true
    }

    /// Returns the replacement package name if any
    pub fn replacement(&self) -> Option<&str> {
        match self {
            Abandoned::Yes => None,
            Abandoned::Replacement(pkg) => Some(pkg.as_str()),
        }
    }
}

/// Author information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Author {
    /// Author name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Author email
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Author homepage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Author role (e.g., "Developer", "Maintainer")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Support information (links to resources)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Support {
    /// Issues/bug tracker URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issues: Option<String>,
    /// Forum URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forum: Option<String>,
    /// Wiki URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wiki: Option<String>,
    /// Source code URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Email address for support
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// IRC channel
    #[serde(skip_serializing_if = "Option::is_none")]
    pub irc: Option<String>,
    /// Documentation URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    /// RSS feed URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss: Option<String>,
    /// Chat URL (e.g., Slack, Discord)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    /// Security policy URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
}

/// Funding information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Funding {
    /// Type of funding (e.g., "github", "patreon", "opencollective")
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub funding_type: Option<String>,
    /// URL to the funding page
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Scripts configuration (composer event handlers)
pub type Scripts = HashMap<String, ScriptHandler>;

/// Script handler which can be a single command or multiple commands
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScriptHandler {
    /// Single script command
    Single(String),
    /// Multiple script commands
    Multiple(Vec<String>),
}

/// Complete package definition
///
/// Represents a Composer package with all metadata, dependencies, and configuration.
/// This combines both the base Package and CompletePackage from PHP Composer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Package {
    /// Package name (lowercase, vendor/package format)
    pub name: String,

    /// Pretty name (original case)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pretty_name: Option<String>,

    /// Normalized version
    pub version: String,

    /// Pretty version (human-readable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pretty_version: Option<String>,

    /// Package type (library, project, metapackage, composer-plugin, etc.)
    #[serde(rename = "type", default = "default_package_type")]
    pub package_type: String,

    /// Package stability
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stability: Option<Stability>,

    /// Source repository information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,

    /// Distribution archive information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dist: Option<Dist>,

    /// Required dependencies
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub require: HashMap<String, String>,

    /// Development dependencies
    #[serde(rename = "require-dev", skip_serializing_if = "HashMap::is_empty", default)]
    pub require_dev: HashMap<String, String>,

    /// Conflicting packages
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub conflict: HashMap<String, String>,

    /// Provided virtual packages
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub provide: HashMap<String, String>,

    /// Replaced packages
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub replace: HashMap<String, String>,

    /// Suggested packages
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub suggest: HashMap<String, String>,

    /// Autoload configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoload: Option<Autoload>,

    /// Development autoload configuration
    #[serde(rename = "autoload-dev", skip_serializing_if = "Option::is_none")]
    pub autoload_dev: Option<Autoload>,

    /// Include paths
    #[serde(rename = "include-path", skip_serializing_if = "Vec::is_empty", default)]
    pub include_path: Vec<String>,

    /// Target directory (deprecated)
    #[serde(rename = "target-dir", skip_serializing_if = "Option::is_none")]
    pub target_dir: Option<String>,

    /// Binary files
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub bin: Vec<String>,

    /// Extra metadata (free-form JSON)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,

    /// Notification URL for package statistics
    #[serde(rename = "notification-url", skip_serializing_if = "Option::is_none")]
    pub notification_url: Option<String>,

    /// Installation source (source or dist)
    #[serde(rename = "installation-source", skip_serializing_if = "Option::is_none")]
    pub installation_source: Option<String>,

    /// Release date
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<DateTime<Utc>>,

    // CompletePackage fields
    /// Package description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Homepage URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,

    /// License identifiers (SPDX format)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub license: Vec<String>,

    /// Keywords
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub keywords: Vec<String>,

    /// Authors
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub authors: Vec<Author>,

    /// Support information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support: Option<Support>,

    /// Funding information
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub funding: Vec<Funding>,

    /// Scripts (composer event handlers)
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub scripts: Scripts,

    /// Whether the package is abandoned
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandoned: Option<Abandoned>,

    /// Archive name pattern
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive: Option<ArchiveConfig>,

    /// Whether this is the default branch
    #[serde(rename = "default-branch", skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<bool>,

    /// Transport options for downloading
    #[serde(rename = "transport-options", skip_serializing_if = "Option::is_none")]
    pub transport_options: Option<serde_json::Value>,
}

/// Archive configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArchiveConfig {
    /// Archive name pattern
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Files/directories to exclude from archive
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub exclude: Vec<String>,
}

fn default_package_type() -> String {
    "library".to_string()
}

/// Package type constants
pub mod package_type {
    /// Standard library package (default)
    pub const LIBRARY: &str = "library";
    /// Project package (not meant to be a dependency)
    pub const PROJECT: &str = "project";
    /// Metapackage - no code, only dependencies
    pub const METAPACKAGE: &str = "metapackage";
    /// Composer plugin
    pub const COMPOSER_PLUGIN: &str = "composer-plugin";
}

impl Package {
    /// Creates a new package with minimal required fields
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        let name = name.into();
        let version = version.into();
        let stability = Stability::from_version(&version);

        Self {
            name: name.to_lowercase(),
            pretty_name: Some(name),
            version: version.clone(),
            pretty_version: Some(version.clone()),
            package_type: default_package_type(),
            stability: Some(stability),
            source: None,
            dist: None,
            require: HashMap::new(),
            require_dev: HashMap::new(),
            conflict: HashMap::new(),
            provide: HashMap::new(),
            replace: HashMap::new(),
            suggest: HashMap::new(),
            autoload: None,
            autoload_dev: None,
            include_path: Vec::new(),
            target_dir: None,
            bin: Vec::new(),
            extra: None,
            notification_url: None,
            installation_source: None,
            time: None,
            description: None,
            homepage: None,
            license: Vec::new(),
            keywords: Vec::new(),
            authors: Vec::new(),
            support: None,
            funding: Vec::new(),
            scripts: HashMap::new(),
            abandoned: None,
            archive: None,
            default_branch: None,
            transport_options: None,
        }
    }

    /// Replace `self.version` constraints with the actual package version.
    ///
    /// In Composer, packages can use `self.version` as a constraint in replace,
    /// provide, conflict, require, and require-dev to reference their own version.
    /// This method replaces all occurrences with `=<version>`.
    pub fn replace_self_version(&mut self) {
        let version_constraint = format!("={}", self.version);

        Self::replace_self_version_in_map(&mut self.replace, &version_constraint);
        Self::replace_self_version_in_map(&mut self.provide, &version_constraint);
        Self::replace_self_version_in_map(&mut self.conflict, &version_constraint);
        Self::replace_self_version_in_map(&mut self.require, &version_constraint);
        Self::replace_self_version_in_map(&mut self.require_dev, &version_constraint);
    }

    /// Helper to replace self.version in a constraint map
    fn replace_self_version_in_map(map: &mut HashMap<String, String>, version_constraint: &str) {
        for constraint in map.values_mut() {
            if constraint == "self.version" {
                *constraint = version_constraint.to_string();
            }
        }
    }

    /// Returns the package name (lowercase)
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the pretty package name (original case)
    pub fn pretty_name(&self) -> &str {
        self.pretty_name.as_deref().unwrap_or(&self.name)
    }

    /// Returns the normalized version
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the pretty version
    pub fn pretty_version(&self) -> &str {
        self.pretty_version.as_deref().unwrap_or(&self.version)
    }

    /// Returns the package type
    pub fn package_type(&self) -> &str {
        &self.package_type
    }

    /// Returns true if this is a metapackage (no files, only dependencies)
    pub fn is_metapackage(&self) -> bool {
        self.package_type == package_type::METAPACKAGE
    }

    /// Returns true if this is a composer plugin
    pub fn is_composer_plugin(&self) -> bool {
        self.package_type == package_type::COMPOSER_PLUGIN
    }

    /// Returns true if this is a platform package (php, ext-*, lib-*)
    pub fn is_platform_package(&self) -> bool {
        self.name == "php"
            || self.name.starts_with("ext-")
            || self.name.starts_with("lib-")
            || self.name == "composer"
            || self.name == "composer-runtime-api"
            || self.name == "composer-plugin-api"
    }

    /// Returns the stability
    pub fn stability(&self) -> Stability {
        self.stability.unwrap_or_default()
    }

    /// Returns true if this is a development version
    pub fn is_dev(&self) -> bool {
        self.stability() == Stability::Dev
    }

    /// Returns true if the package is abandoned
    pub fn is_abandoned(&self) -> bool {
        self.abandoned.is_some()
    }

    /// Returns the unique name (name-version)
    pub fn unique_name(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }

    /// Returns a pretty string representation
    pub fn pretty_string(&self) -> String {
        format!("{} {}", self.pretty_name(), self.pretty_version())
    }

    /// Converts require/require-dev/etc maps to Link structs
    pub fn get_links(&self) -> Vec<Link> {
        use super::LinkType;

        let mut links = Vec::new();

        for (target, constraint) in &self.require {
            links.push(Link::new(
                &self.name,
                target,
                constraint,
                LinkType::Require,
            ));
        }

        for (target, constraint) in &self.require_dev {
            links.push(Link::new(
                &self.name,
                target,
                constraint,
                LinkType::DevRequire,
            ));
        }

        for (target, constraint) in &self.conflict {
            links.push(Link::new(
                &self.name,
                target,
                constraint,
                LinkType::Conflict,
            ));
        }

        for (target, constraint) in &self.provide {
            links.push(Link::new(
                &self.name,
                target,
                constraint,
                LinkType::Provide,
            ));
        }

        for (target, constraint) in &self.replace {
            links.push(Link::new(
                &self.name,
                target,
                constraint,
                LinkType::Replace,
            ));
        }

        links
    }

    /// Returns all names this package "owns" - its name plus all provides and replaces
    ///
    /// When `include_provides` is true, includes both provides and replaces.
    /// When false, only includes the package name and replaces (replaces are stronger).
    ///
    /// This is used for:
    /// - Pool indexing (finding packages by any of their names)
    /// - Same-name conflict detection (packages providing same name conflict)
    pub fn get_names(&self, include_provides: bool) -> Vec<String> {
        let mut names = vec![self.name.to_lowercase()];

        // Replaces are always included (stronger relationship)
        for (replaced_name, _) in &self.replace {
            let name = replaced_name.to_lowercase();
            if !names.contains(&name) {
                names.push(name);
            }
        }

        // Provides are only included when requested
        if include_provides {
            for (provided_name, _) in &self.provide {
                let name = provided_name.to_lowercase();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }

        names
    }

    /// Updates both source and dist references (for version control)
    pub fn set_references(&mut self, reference: impl Into<String>) {
        let reference = reference.into();

        if let Some(source) = &mut self.source {
            source.reference = reference.clone();
        }

        if let Some(dist) = &mut self.dist {
            // Only update dist reference for GitHub/GitLab/Bitbucket URLs
            let url = dist.url.to_lowercase();
            if url.contains("github.com")
                || url.contains("gitlab.com")
                || url.contains("bitbucket.org")
            {
                dist.reference = Some(reference);
            }
        }
    }
}

impl Default for Package {
    fn default() -> Self {
        Self::new("vendor/package", "1.0.0")
    }
}

impl std::fmt::Display for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.unique_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_creation() {
        let package = Package::new("vendor/package", "1.0.0");

        assert_eq!(package.name(), "vendor/package");
        assert_eq!(package.version(), "1.0.0");
        assert_eq!(package.stability(), Stability::Stable);
    }

    #[test]
    fn test_package_dev_version() {
        let package = Package::new("vendor/package", "dev-main");

        assert!(package.is_dev());
        assert_eq!(package.stability(), Stability::Dev);
    }

    #[test]
    fn test_stability_priority() {
        assert!(Stability::Stable.priority() < Stability::RC.priority());
        assert!(Stability::RC.priority() < Stability::Beta.priority());
        assert!(Stability::Beta.priority() < Stability::Alpha.priority());
        assert!(Stability::Alpha.priority() < Stability::Dev.priority());
    }

    #[test]
    fn test_stability_from_str() {
        use std::str::FromStr;

        assert_eq!(Stability::from_str("dev").unwrap(), Stability::Dev);
        assert_eq!(Stability::from_str("alpha").unwrap(), Stability::Alpha);
        assert_eq!(Stability::from_str("beta").unwrap(), Stability::Beta);
        assert_eq!(Stability::from_str("rc").unwrap(), Stability::RC);
        assert_eq!(Stability::from_str("RC").unwrap(), Stability::RC);
        assert_eq!(Stability::from_str("stable").unwrap(), Stability::Stable);
        assert_eq!(Stability::from_str("STABLE").unwrap(), Stability::Stable);
        assert_eq!(Stability::from_str("").unwrap(), Stability::Stable);
        assert_eq!(Stability::from_str("unknown").unwrap(), Stability::Stable);
    }

    #[test]
    fn test_abandoned_package() {
        let mut package = Package::new("vendor/old-package", "1.0.0");
        package.abandoned = Some(Abandoned::Replacement("vendor/new-package".to_string()));

        assert!(package.is_abandoned());
        assert_eq!(
            package.abandoned.as_ref().unwrap().replacement(),
            Some("vendor/new-package")
        );
    }

    #[test]
    fn test_package_serialization() {
        let package = Package::new("vendor/package", "1.0.0");
        let json = serde_json::to_string(&package).unwrap();
        let deserialized: Package = serde_json::from_str(&json).unwrap();

        assert_eq!(package.name(), deserialized.name());
        assert_eq!(package.version(), deserialized.version());
    }

    #[test]
    fn test_pretty_version_defaults_to_version() {
        let package = Package::new("vendor/package", "1.0.0.0");
        assert_eq!(package.pretty_version(), "1.0.0.0");
    }

    #[test]
    fn test_pretty_version_with_explicit_value() {
        let mut package = Package::new("vendor/package", "1.0.0.0");
        package.pretty_version = Some("v1.0.0".to_string());
        assert_eq!(package.pretty_version(), "v1.0.0");
        assert_eq!(package.version(), "1.0.0.0");
    }

    #[test]
    fn test_pretty_version_formats() {
        let test_cases = [
            ("1.0.0.0", "1.0.0"),
            ("1.0.0.0", "v1.0.0"),
            ("2.3.4.0", "2.3.4"),
            ("1.0.0.0", "1.0"),
            ("9999999.0.0.0-dev", "dev-main"),
        ];

        for (normalized, pretty) in test_cases {
            let mut package = Package::new("vendor/package", normalized);
            package.pretty_version = Some(pretty.to_string());
            assert_eq!(package.pretty_version(), pretty);
        }
    }
}
