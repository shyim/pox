use super::{Link, LinkType, Package, Stability};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::Arc;

/// Represents a version alias for a package
///
/// An alias package wraps another package and presents a different version,
/// while delegating most functionality to the aliased package. This is used for:
///
/// 1. Branch aliases: e.g., `dev-main` aliased as `1.0.x-dev` via `extra.branch-alias`
/// 2. Root aliases: aliases defined in the root package's `require` constraints
///    using the `as` syntax, e.g., `"vendor/package": "dev-main as 1.0.0"`
///
/// The alias package has its own version but inherits all other properties
/// (autoload, source, dist, etc.) from the aliased package.
#[derive(Debug, Clone)]
pub struct AliasPackage {
    /// The package this is an alias of
    alias_of: Arc<Package>,

    /// Normalized alias version
    version: String,

    /// Pretty alias version for display
    pretty_version: String,

    /// Stability of the alias version
    stability: Stability,

    /// Whether this is a development version
    is_dev: bool,

    /// Whether this alias was created from the root package requirements
    /// (using "as" syntax in require constraints)
    is_root_package_alias: bool,

    /// Whether this package has self.version requirements that were replaced
    has_self_version_requires: bool,

    /// Transformed require dependencies (with self.version replaced)
    require: IndexMap<String, String>,

    /// Transformed dev require dependencies (with self.version replaced)
    require_dev: IndexMap<String, String>,

    /// Transformed conflict dependencies (with self.version replaced)
    conflict: IndexMap<String, String>,

    /// Transformed provide dependencies (with self.version replaced)
    provide: IndexMap<String, String>,

    /// Transformed replace dependencies (with self.version replaced)
    replace: IndexMap<String, String>,
}

impl AliasPackage {
    /// Creates a new alias package
    ///
    /// # Arguments
    /// * `alias_of` - The package this is an alias of
    /// * `version` - The normalized alias version
    /// * `pretty_version` - The pretty version for display
    pub fn new(alias_of: Arc<Package>, version: String, pretty_version: String) -> Self {
        let stability = Stability::from_version(&version);
        let is_dev = stability == Stability::Dev;

        let mut alias = Self {
            alias_of: alias_of.clone(),
            version: version.clone(),
            pretty_version: pretty_version.clone(),
            stability,
            is_dev,
            is_root_package_alias: false,
            has_self_version_requires: false,
            require: IndexMap::new(),
            require_dev: IndexMap::new(),
            conflict: IndexMap::new(),
            provide: IndexMap::new(),
            replace: IndexMap::new(),
        };

        // Transform dependencies by replacing self.version constraints
        alias.transform_dependencies(&alias_of, &version, &pretty_version);

        alias
    }

    /// Transform dependencies by replacing self.version constraints with the alias version
    fn transform_dependencies(
        &mut self,
        alias_of: &Package,
        version: &str,
        pretty_version: &str,
    ) {
        // For require and require-dev, replace self.version constraints
        self.require = Self::replace_self_version_deps(
            &alias_of.require,
            version,
            pretty_version,
            false,
            &mut self.has_self_version_requires,
        );

        self.require_dev = Self::replace_self_version_deps(
            &alias_of.require_dev,
            version,
            pretty_version,
            false,
            &mut false,
        );

        // For conflict, provide, replace - we need to add the alias version as well
        self.conflict = Self::replace_self_version_deps(
            &alias_of.conflict,
            version,
            pretty_version,
            true,
            &mut false,
        );

        self.provide = Self::replace_self_version_deps(
            &alias_of.provide,
            version,
            pretty_version,
            true,
            &mut false,
        );

        self.replace = Self::replace_self_version_deps(
            &alias_of.replace,
            version,
            pretty_version,
            true,
            &mut false,
        );
    }

    /// Replace self.version constraints in dependencies
    ///
    /// For conflict/provide/replace (is_link_type = true), we add new entries
    /// rather than replacing, so both versions are included.
    fn replace_self_version_deps(
        deps: &IndexMap<String, String>,
        version: &str,
        _pretty_version: &str,
        add_alias_entries: bool,
        has_self_version: &mut bool,
    ) -> IndexMap<String, String> {
        let mut result = IndexMap::new();

        for (target, constraint) in deps {
            if constraint == "self.version" {
                *has_self_version = true;
                // Replace self.version with the alias version
                result.insert(target.clone(), format!("={}", version));

                if add_alias_entries {
                    // For conflict/provide/replace, also keep original entry
                    // by adding a unique key (this is a simplification; in practice
                    // the solver handles this differently)
                }
            } else {
                result.insert(target.clone(), constraint.clone());
            }
        }

        // If adding alias entries, also copy originals that weren't self.version
        if add_alias_entries {
            for (target, constraint) in deps {
                if constraint != "self.version" && !result.contains_key(target) {
                    result.insert(target.clone(), constraint.clone());
                }
            }
        }

        result
    }

    /// Returns the package this is an alias of
    pub fn alias_of(&self) -> &Package {
        &self.alias_of
    }

    /// Returns the aliased package as an Arc
    pub fn alias_of_arc(&self) -> Arc<Package> {
        Arc::clone(&self.alias_of)
    }

    /// Returns the alias version (normalized)
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the pretty alias version
    pub fn pretty_version(&self) -> &str {
        &self.pretty_version
    }

    /// Returns the stability of the alias
    pub fn stability(&self) -> Stability {
        self.stability
    }

    /// Returns true if this is a development version
    pub fn is_dev(&self) -> bool {
        self.is_dev
    }

    /// Sets whether this alias was created from root package requirements
    pub fn set_root_package_alias(&mut self, value: bool) {
        self.is_root_package_alias = value;
    }

    /// Returns true if this alias was created from root package requirements
    pub fn is_root_package_alias(&self) -> bool {
        self.is_root_package_alias
    }

    /// Returns true if this package had self.version requirements that were replaced
    pub fn has_self_version_requires(&self) -> bool {
        self.has_self_version_requires
    }

    // Delegated properties from the aliased package

    /// Returns the package name
    pub fn name(&self) -> &str {
        self.alias_of.name()
    }

    /// Returns the pretty package name
    pub fn pretty_name(&self) -> &str {
        self.alias_of.pretty_name()
    }

    /// Returns the package type
    pub fn package_type(&self) -> &str {
        self.alias_of.package_type()
    }

    /// Returns the unique name (name-version)
    pub fn unique_name(&self) -> String {
        format!("{}-{}", self.name(), self.version)
    }

    /// Returns a pretty string representation
    pub fn pretty_string(&self) -> String {
        format!("{} {}", self.pretty_name(), self.pretty_version())
    }

    /// Returns the require dependencies (with self.version replaced)
    pub fn require(&self) -> &IndexMap<String, String> {
        &self.require
    }

    /// Returns the dev require dependencies (with self.version replaced)
    pub fn require_dev(&self) -> &IndexMap<String, String> {
        &self.require_dev
    }

    /// Returns the conflict dependencies (with self.version replaced)
    pub fn conflict(&self) -> &IndexMap<String, String> {
        &self.conflict
    }

    /// Returns the provide dependencies (with self.version replaced)
    pub fn provide(&self) -> &IndexMap<String, String> {
        &self.provide
    }

    /// Returns the replace dependencies (with self.version replaced)
    pub fn replace(&self) -> &IndexMap<String, String> {
        &self.replace
    }

    /// Converts require/require-dev/etc maps to Link structs
    pub fn get_links(&self) -> Vec<Link> {
        let mut links = Vec::new();

        for (target, constraint) in &self.require {
            links.push(Link::new(
                self.name(),
                target,
                constraint,
                LinkType::Require,
            ));
        }

        for (target, constraint) in &self.require_dev {
            links.push(Link::new(
                self.name(),
                target,
                constraint,
                LinkType::DevRequire,
            ));
        }

        for (target, constraint) in &self.conflict {
            links.push(Link::new(
                self.name(),
                target,
                constraint,
                LinkType::Conflict,
            ));
        }

        for (target, constraint) in &self.provide {
            links.push(Link::new(
                self.name(),
                target,
                constraint,
                LinkType::Provide,
            ));
        }

        for (target, constraint) in &self.replace {
            links.push(Link::new(
                self.name(),
                target,
                constraint,
                LinkType::Replace,
            ));
        }

        links
    }
}

impl std::fmt::Display for AliasPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}alias of {})",
            self.unique_name(),
            if self.is_root_package_alias { "root " } else { "" },
            self.alias_of.version()
        )
    }
}

/// Default branch alias constant (used for dev-master/dev-main)
pub const DEFAULT_BRANCH_ALIAS: &str = "9999999-dev";

/// Parses branch aliases from a package's extra.branch-alias configuration
///
/// Branch aliases allow packages to map development branches to semantic versions.
/// For example: `"dev-main": "1.0.x-dev"` makes `dev-main` appear as `1.0.x-dev`.
///
/// # Arguments
/// * `extra` - The package's extra configuration
///
/// # Returns
/// A map of source version to (alias_normalized, alias_pretty)
pub fn parse_branch_aliases(
    extra: Option<&serde_json::Value>,
) -> HashMap<String, (String, String)> {
    let mut aliases = HashMap::new();

    let Some(extra) = extra else {
        return aliases;
    };

    let Some(branch_alias) = extra.get("branch-alias") else {
        return aliases;
    };

    let Some(branch_alias) = branch_alias.as_object() else {
        return aliases;
    };

    for (source_branch, target_branch) in branch_alias {
        let Some(target_branch) = target_branch.as_str() else {
            continue;
        };

        // Ensure it's an alias to a -dev package
        if !target_branch.ends_with("-dev") {
            continue;
        }

        // Normalize the source branch
        let source_normalized = normalize_branch(source_branch);

        // Handle the target branch
        let (alias_normalized, alias_pretty) = if target_branch == DEFAULT_BRANCH_ALIAS {
            (DEFAULT_BRANCH_ALIAS.to_string(), target_branch.to_string())
        } else {
            // Normalize without -dev suffix
            let without_dev = &target_branch[..target_branch.len() - 4];
            let normalized = normalize_branch(without_dev);

            // Ensure normalized version ends with -dev
            if !normalized.ends_with("-dev") {
                continue;
            }

            (normalized, target_branch.to_string())
        };

        aliases.insert(source_normalized, (alias_normalized, alias_pretty));
    }

    aliases
}

/// Normalizes a branch name to a version
fn normalize_branch(branch: &str) -> String {
    let branch = branch.trim();

    // Remove "dev-" prefix if present
    let branch = branch.strip_prefix("dev-").unwrap_or(branch);

    // Common branch name mappings
    match branch.to_lowercase().as_str() {
        "master" | "main" | "trunk" | "default" => format!("dev-{}", branch),
        _ => {
            // Check if it looks like a version
            if branch.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                // Numeric branch like "1.0" -> "1.0.x-dev"
                format!("{}.x-dev", branch.trim_end_matches(".x").trim_end_matches(".0"))
            } else {
                format!("dev-{}", branch)
            }
        }
    }
}

/// Parses inline aliases from require constraints
///
/// Composer allows specifying aliases inline in require constraints using "as":
/// `"vendor/package": "dev-main as 1.0.0"`
///
/// # Arguments
/// * `constraint` - The version constraint string
///
/// # Returns
/// `Some((actual_constraint, alias_version))` if an alias is present, `None` otherwise
pub fn parse_inline_alias(constraint: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = constraint.split(" as ").collect();
    if parts.len() == 2 {
        let actual = parts[0].trim().to_string();
        let alias = parts[1].trim().to_string();
        Some((actual, alias))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alias_package_creation() {
        let package = Package::new("vendor/package", "dev-main");
        let alias = AliasPackage::new(
            Arc::new(package),
            "1.0.0.0".to_string(),
            "1.0.0".to_string(),
        );

        assert_eq!(alias.name(), "vendor/package");
        assert_eq!(alias.version(), "1.0.0.0");
        assert_eq!(alias.pretty_version(), "1.0.0");
        assert_eq!(alias.alias_of().version(), "dev-main");
        assert!(!alias.is_root_package_alias());
    }

    #[test]
    fn test_alias_package_root_alias() {
        let package = Package::new("vendor/package", "dev-main");
        let mut alias = AliasPackage::new(
            Arc::new(package),
            "1.0.0.0".to_string(),
            "1.0.0".to_string(),
        );

        alias.set_root_package_alias(true);
        assert!(alias.is_root_package_alias());
    }

    #[test]
    fn test_alias_package_display() {
        let package = Package::new("vendor/package", "dev-main");
        let alias = AliasPackage::new(
            Arc::new(package),
            "1.0.0.0".to_string(),
            "1.0.0".to_string(),
        );

        let display = alias.to_string();
        assert!(display.contains("vendor/package"));
        assert!(display.contains("1.0.0.0"));
        assert!(display.contains("alias of"));
        assert!(display.contains("dev-main"));
    }

    #[test]
    fn test_alias_package_root_display() {
        let package = Package::new("vendor/package", "dev-main");
        let mut alias = AliasPackage::new(
            Arc::new(package),
            "1.0.0.0".to_string(),
            "1.0.0".to_string(),
        );
        alias.set_root_package_alias(true);

        let display = alias.to_string();
        assert!(display.contains("root alias of"));
    }

    #[test]
    fn test_alias_stability() {
        let package = Package::new("vendor/package", "dev-main");
        let stable_alias = AliasPackage::new(
            Arc::new(package.clone()),
            "1.0.0.0".to_string(),
            "1.0.0".to_string(),
        );
        assert_eq!(stable_alias.stability(), Stability::Stable);
        assert!(!stable_alias.is_dev());

        let dev_alias = AliasPackage::new(
            Arc::new(package),
            "1.0.x-dev".to_string(),
            "1.0.x-dev".to_string(),
        );
        assert_eq!(dev_alias.stability(), Stability::Dev);
        assert!(dev_alias.is_dev());
    }

    #[test]
    fn test_self_version_replacement() {
        let mut package = Package::new("vendor/package", "dev-main");
        package.require.insert("other/package".to_string(), "self.version".to_string());

        let alias = AliasPackage::new(
            Arc::new(package),
            "1.0.0.0".to_string(),
            "1.0.0".to_string(),
        );

        assert!(alias.has_self_version_requires());
        assert_eq!(alias.require().get("other/package"), Some(&"=1.0.0.0".to_string()));
    }

    #[test]
    fn test_parse_inline_alias() {
        assert_eq!(
            parse_inline_alias("dev-main as 1.0.0"),
            Some(("dev-main".to_string(), "1.0.0".to_string()))
        );

        assert_eq!(
            parse_inline_alias("dev-feature as 2.0.x-dev"),
            Some(("dev-feature".to_string(), "2.0.x-dev".to_string()))
        );

        assert_eq!(parse_inline_alias("^1.0"), None);
        assert_eq!(parse_inline_alias(">=1.0,<2.0"), None);
    }

    #[test]
    fn test_parse_branch_aliases() {
        let extra = serde_json::json!({
            "branch-alias": {
                "dev-main": "1.0.x-dev",
                "dev-2.0": "2.0.x-dev"
            }
        });

        let aliases = parse_branch_aliases(Some(&extra));
        assert!(!aliases.is_empty());
    }

    #[test]
    fn test_parse_branch_aliases_empty() {
        let aliases = parse_branch_aliases(None);
        assert!(aliases.is_empty());

        let extra = serde_json::json!({});
        let aliases = parse_branch_aliases(Some(&extra));
        assert!(aliases.is_empty());
    }
}
