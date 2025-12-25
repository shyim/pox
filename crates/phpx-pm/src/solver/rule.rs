use std::fmt;
use std::hash::{Hash, Hasher};

use super::pool::PackageId;

/// A literal in SAT terms - positive means "install", negative means "don't install"
pub type Literal = i32;

/// Types of rules generated during dependency resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleType {
    /// Root composer.json requirement
    RootRequire,
    /// Fixed package that must stay installed (e.g., platform packages)
    Fixed,
    /// Package dependency: if A is installed, then B|C|D must be installed
    PackageRequires,
    /// Package conflict: A and B cannot both be installed
    PackageConflict,
    /// Multiple versions of same package: only one can be installed (binary conflict)
    PackageSameName,
    /// Multiple versions of same package: at most one can be installed (n-ary multi-conflict)
    /// This is more efficient than O(n²) binary conflicts for packages with many versions
    MultiConflict,
    /// Alias must require its target
    PackageAlias,
    /// Target must require its alias
    PackageInverseAlias,
    /// Learned clause from conflict analysis
    Learned,
}

impl RuleType {
    /// Get the priority of this rule type (lower = higher priority)
    pub fn priority(&self) -> u8 {
        match self {
            RuleType::RootRequire | RuleType::Fixed => 1, // Request rules
            RuleType::PackageRequires
            | RuleType::PackageConflict
            | RuleType::PackageSameName
            | RuleType::MultiConflict
            | RuleType::PackageAlias
            | RuleType::PackageInverseAlias => 0, // Package rules
            RuleType::Learned => 4,
        }
    }

    /// Check if this is a multi-conflict rule type
    pub fn is_multi_conflict(&self) -> bool {
        matches!(self, RuleType::MultiConflict)
    }
}

/// A SAT rule (clause) representing a dependency constraint.
///
/// Rules are disjunctions (OR) of literals. A rule is satisfied when
/// at least one of its literals is true.
///
/// # Examples
///
/// - `[A]` - Package A must be installed (assertion)
/// - `[-A]` - Package A must not be installed
/// - `[-A, B, C]` - If A is installed, then B or C must be installed
/// - `[-A, -B]` - A and B cannot both be installed (conflict)
#[derive(Clone)]
pub struct Rule {
    /// The literals in this rule
    literals: Vec<Literal>,
    /// Type of rule
    rule_type: RuleType,
    /// Rule ID (assigned by RuleSet)
    id: u32,
    /// Source package ID (for error messages)
    source_package: Option<PackageId>,
    /// Target package name (for error messages)
    target_name: Option<String>,
    /// Constraint string (for error messages)
    constraint: Option<String>,
    /// Whether this rule is disabled
    disabled: bool,
}

impl Rule {
    /// Create a new rule with the given literals
    pub fn new(literals: Vec<Literal>, rule_type: RuleType) -> Self {
        Self {
            literals,
            rule_type,
            id: 0,
            source_package: None,
            target_name: None,
            constraint: None,
            disabled: false,
        }
    }

    /// Create an assertion rule (single literal that must be true)
    pub fn assertion(literal: Literal, rule_type: RuleType) -> Self {
        Self::new(vec![literal], rule_type)
    }

    /// Create a requirement rule: if source is installed, one of targets must be
    pub fn requires(source: PackageId, targets: Vec<PackageId>) -> Self {
        let mut literals = vec![-source];
        literals.extend(targets);
        Self::new(literals, RuleType::PackageRequires)
    }

    /// Create a conflict rule: these packages cannot all be installed together
    pub fn conflict(packages: Vec<PackageId>) -> Self {
        let literals: Vec<_> = packages.into_iter().map(|p| -p).collect();
        Self::new(literals, RuleType::PackageConflict)
    }

    /// Create a same-name rule: only one of these versions can be installed (binary conflict)
    pub fn same_name(packages: Vec<PackageId>) -> Self {
        let literals: Vec<_> = packages.into_iter().map(|p| -p).collect();
        Self::new(literals, RuleType::PackageSameName)
    }

    /// Create a multi-conflict rule: at most one of these packages can be installed
    /// This is more efficient than O(n²) binary conflicts for packages with many versions.
    /// The rule watches all literals and triggers when any becomes true.
    pub fn multi_conflict(packages: Vec<PackageId>) -> Self {
        let literals: Vec<_> = packages.into_iter().map(|p| -p).collect();
        Self::new(literals, RuleType::MultiConflict)
    }

    /// Check if this is a multi-conflict rule
    pub fn is_multi_conflict(&self) -> bool {
        self.rule_type.is_multi_conflict()
    }

    /// Create a root requirement rule
    pub fn root_require(targets: Vec<PackageId>) -> Self {
        Self::new(targets, RuleType::RootRequire)
    }

    /// Create a fixed package rule
    pub fn fixed(package: PackageId) -> Self {
        Self::assertion(package, RuleType::Fixed)
    }

    /// Create a learned rule from conflict analysis
    pub fn learned(literals: Vec<Literal>) -> Self {
        Self::new(literals, RuleType::Learned)
    }

    /// Set the rule ID
    pub fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    /// Get the rule ID
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Set source package for error messages
    pub fn with_source(mut self, package: PackageId) -> Self {
        self.source_package = Some(package);
        self
    }

    /// Set target name for error messages
    pub fn with_target(mut self, name: impl Into<String>) -> Self {
        self.target_name = Some(name.into());
        self
    }

    /// Set constraint for error messages
    pub fn with_constraint(mut self, constraint: impl Into<String>) -> Self {
        self.constraint = Some(constraint.into());
        self
    }

    /// Get the rule type
    pub fn rule_type(&self) -> RuleType {
        self.rule_type
    }

    /// Get the literals
    pub fn literals(&self) -> &[Literal] {
        &self.literals
    }

    /// Get a mutable reference to literals
    pub fn literals_mut(&mut self) -> &mut Vec<Literal> {
        &mut self.literals
    }

    /// Get source package ID
    pub fn source_package(&self) -> Option<PackageId> {
        self.source_package
    }

    /// Get target name
    pub fn target_name(&self) -> Option<&str> {
        self.target_name.as_deref()
    }

    /// Get constraint
    pub fn constraint(&self) -> Option<&str> {
        self.constraint.as_deref()
    }

    /// Check if this is an assertion (single literal)
    pub fn is_assertion(&self) -> bool {
        self.literals.len() == 1
    }

    /// Check if this rule is disabled
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Disable this rule
    pub fn disable(&mut self) {
        self.disabled = true;
    }

    /// Enable this rule
    pub fn enable(&mut self) {
        self.disabled = false;
    }

    /// Get the number of literals
    pub fn len(&self) -> usize {
        self.literals.len()
    }

    /// Check if the rule is empty
    pub fn is_empty(&self) -> bool {
        self.literals.is_empty()
    }

    /// Get a hash of this rule's literals for deduplication
    pub fn literal_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();

        // Sort literals for consistent hashing
        let mut sorted = self.literals.clone();
        sorted.sort();
        sorted.hash(&mut hasher);

        hasher.finish()
    }

    /// Check if two rules have the same literals (regardless of order)
    pub fn equals_literals(&self, other: &Rule) -> bool {
        if self.literals.len() != other.literals.len() {
            return false;
        }

        let mut a = self.literals.clone();
        let mut b = other.literals.clone();
        a.sort();
        b.sort();
        a == b
    }
}

impl fmt::Debug for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rule({:?}, {:?})", self.rule_type, self.literals)
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let literals: Vec<String> = self.literals
            .iter()
            .map(|&l| {
                if l > 0 {
                    format!("+{}", l)
                } else {
                    format!("{}", l)
                }
            })
            .collect();

        write!(f, "({}) [{}]", self.rule_type_str(), literals.join(" | "))
    }
}

impl Rule {
    fn rule_type_str(&self) -> &'static str {
        match self.rule_type {
            RuleType::RootRequire => "root-require",
            RuleType::Fixed => "fixed",
            RuleType::PackageRequires => "requires",
            RuleType::PackageConflict => "conflict",
            RuleType::PackageSameName => "same-name",
            RuleType::MultiConflict => "multi-conflict",
            RuleType::PackageAlias => "alias",
            RuleType::PackageInverseAlias => "inverse-alias",
            RuleType::Learned => "learned",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_assertion() {
        let rule = Rule::assertion(5, RuleType::Fixed);
        assert!(rule.is_assertion());
        assert_eq!(rule.literals(), &[5]);
    }

    #[test]
    fn test_rule_requires() {
        let rule = Rule::requires(1, vec![2, 3, 4]);
        assert_eq!(rule.literals(), &[-1, 2, 3, 4]);
        assert_eq!(rule.rule_type(), RuleType::PackageRequires);
    }

    #[test]
    fn test_rule_conflict() {
        let rule = Rule::conflict(vec![1, 2]);
        assert_eq!(rule.literals(), &[-1, -2]);
        assert_eq!(rule.rule_type(), RuleType::PackageConflict);
    }

    #[test]
    fn test_rule_same_name() {
        let rule = Rule::same_name(vec![1, 2, 3]);
        assert_eq!(rule.literals(), &[-1, -2, -3]);
        assert_eq!(rule.rule_type(), RuleType::PackageSameName);
    }

    #[test]
    fn test_rule_literal_hash() {
        let rule1 = Rule::new(vec![1, 2, 3], RuleType::PackageRequires);
        let rule2 = Rule::new(vec![3, 1, 2], RuleType::PackageRequires);
        let rule3 = Rule::new(vec![1, 2, 4], RuleType::PackageRequires);

        assert_eq!(rule1.literal_hash(), rule2.literal_hash());
        assert_ne!(rule1.literal_hash(), rule3.literal_hash());
    }

    #[test]
    fn test_rule_equals_literals() {
        let rule1 = Rule::new(vec![1, 2, 3], RuleType::PackageRequires);
        let rule2 = Rule::new(vec![3, 1, 2], RuleType::PackageConflict);
        let rule3 = Rule::new(vec![1, 2], RuleType::PackageRequires);

        assert!(rule1.equals_literals(&rule2));
        assert!(!rule1.equals_literals(&rule3));
    }

    #[test]
    fn test_rule_display() {
        let rule = Rule::requires(1, vec![2, 3]);
        let display = format!("{}", rule);
        assert!(display.contains("requires"));
    }
}
