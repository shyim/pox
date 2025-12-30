use serde::{Deserialize, Serialize};
use std::fmt;

/// Type of package link
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkType {
    /// Regular require dependency
    #[serde(rename = "requires")]
    Require,
    /// Development dependency
    #[serde(rename = "devRequires")]
    DevRequire,
    /// Package provides this virtual package
    #[serde(rename = "provides")]
    Provide,
    /// Conflicts with this package
    #[serde(rename = "conflicts")]
    Conflict,
    /// Replaces this package
    #[serde(rename = "replaces")]
    Replace,
}

impl LinkType {
    /// Returns a human-readable description of the link type
    pub fn description(&self) -> &'static str {
        match self {
            LinkType::Require => "requires",
            LinkType::DevRequire => "requires (for development)",
            LinkType::Provide => "provides",
            LinkType::Conflict => "conflicts",
            LinkType::Replace => "replaces",
        }
    }
}

impl fmt::Display for LinkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// Represents a link between two packages
///
/// A link connects a source package to a target package with a version constraint.
/// This is used for various dependency types (require, conflict, provide, etc.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// Source package name (lowercase)
    pub source: String,
    /// Target package name (lowercase)
    pub target: String,
    /// Version constraint string (e.g., "^1.0", ">=2.0,<3.0")
    pub constraint: String,
    /// Pretty constraint string for display (same as constraint in most cases)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pretty_constraint: Option<String>,
    /// Type of link (require, conflict, etc.)
    #[serde(rename = "type")]
    pub link_type: LinkType,
}

impl Link {
    /// Creates a new link between two packages
    pub fn new(
        source: impl Into<String>,
        target: impl Into<String>,
        constraint: impl Into<String>,
        link_type: LinkType,
    ) -> Self {
        let source = source.into().to_lowercase();
        let target = target.into().to_lowercase();
        let constraint = constraint.into();

        Self {
            source,
            target,
            pretty_constraint: Some(constraint.clone()),
            constraint,
            link_type,
        }
    }

    /// Returns the constraint string to use for display
    pub fn pretty_constraint(&self) -> &str {
        self.pretty_constraint.as_deref().unwrap_or(&self.constraint)
    }

    /// Returns a human-readable description of this link
    pub fn description(&self) -> String {
        format!(
            "{} {} {} ({})",
            self.source,
            self.link_type.description(),
            self.target,
            self.pretty_constraint()
        )
    }
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

impl Default for Link {
    fn default() -> Self {
        Self {
            source: String::new(),
            target: String::new(),
            constraint: "*".to_string(),
            pretty_constraint: Some("*".to_string()),
            link_type: LinkType::Require,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_creation() {
        let link = Link::new("my/package", "vendor/library", "^1.0", LinkType::Require);

        assert_eq!(link.source, "my/package");
        assert_eq!(link.target, "vendor/library");
        assert_eq!(link.constraint, "^1.0");
        assert_eq!(link.link_type, LinkType::Require);
    }

    #[test]
    fn test_link_display() {
        let link = Link::new("my/package", "vendor/library", "^1.0", LinkType::Require);
        let display = link.to_string();

        assert!(display.contains("my/package"));
        assert!(display.contains("vendor/library"));
        assert!(display.contains("requires"));
        assert!(display.contains("^1.0"));
    }

    #[test]
    fn test_link_type_description() {
        assert_eq!(LinkType::Require.description(), "requires");
        assert_eq!(LinkType::DevRequire.description(), "requires (for development)");
        assert_eq!(LinkType::Provide.description(), "provides");
        assert_eq!(LinkType::Conflict.description(), "conflicts");
        assert_eq!(LinkType::Replace.description(), "replaces");
    }
}
