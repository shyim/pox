use std::fmt;

use super::pool::{Pool, PackageId};
use super::rule::{Rule, RuleType};

/// A problem encountered during dependency resolution.
///
/// Problems explain why a solution cannot be found.
#[derive(Debug, Clone)]
pub struct Problem {
    /// Rules involved in this problem
    pub rules: Vec<ProblemRule>,
    /// Human-readable explanation
    pub message: Option<String>,
}

/// A rule that contributes to a problem
#[derive(Debug, Clone)]
pub struct ProblemRule {
    /// The rule ID
    pub rule_id: u32,
    /// Rule type
    pub rule_type: RuleType,
    /// Source package ID (deprecated, use source_name instead)
    pub source: Option<PackageId>,
    /// Source package name and version (resolved at problem creation time)
    pub source_name: Option<String>,
    /// Target package name
    pub target: Option<String>,
    /// Constraint
    pub constraint: Option<String>,
}

impl Problem {
    /// Create a new problem
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            message: None,
        }
    }

    /// Add a rule to this problem (without resolving source package name)
    pub fn add_rule(&mut self, rule: &Rule) {
        self.rules.push(ProblemRule {
            rule_id: rule.id(),
            rule_type: rule.rule_type(),
            source: rule.source_package(),
            source_name: None,
            target: rule.target_name().map(String::from),
            constraint: rule.constraint().map(String::from),
        });
    }

    /// Add a rule to this problem, resolving source package name from the pool
    pub fn add_rule_with_pool(&mut self, rule: &Rule, pool: &Pool) {
        let source_name = rule.source_package()
            .and_then(|id| pool.package(id))
            .map(|p| p.pretty_string());

        self.rules.push(ProblemRule {
            rule_id: rule.id(),
            rule_type: rule.rule_type(),
            source: rule.source_package(),
            source_name,
            target: rule.target_name().map(String::from),
            constraint: rule.constraint().map(String::from),
        });
    }

    /// Set a custom message
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Generate a human-readable description of this problem
    pub fn describe(&self, pool: &Pool) -> String {
        let mut lines = Vec::new();

        for rule in &self.rules {
            let line = describe_rule(pool, rule);
            if !line.is_empty() {
                lines.push(format!("  - {}", line));
            }
        }

        if let Some(ref msg) = self.message {
            format!("{}\n{}", msg, lines.join("\n"))
        } else {
            lines.join("\n")
        }
    }
}

impl Default for Problem {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to get source package name - prefer pre-resolved name, fall back to pool lookup
fn get_source_name(rule: &ProblemRule, pool: &Pool) -> String {
    rule.source_name.clone().unwrap_or_else(|| {
        rule.source
            .and_then(|id| pool.package(id))
            .map(|p| p.pretty_string())
            .unwrap_or_else(|| "unknown".to_string())
    })
}

/// Describe a problem rule in human-readable form
fn describe_rule(pool: &Pool, rule: &ProblemRule) -> String {
    match rule.rule_type {
        RuleType::RootRequire => {
            let target = rule.target.as_deref().unwrap_or("unknown");
            let constraint = rule.constraint.as_deref().unwrap_or("*");
            // Check if any packages exist for this requirement
            let has_packages = !pool.packages_by_name(target).is_empty();
            if has_packages {
                format!(
                    "Root composer.json requires {} {}, but no version satisfying the constraint can be installed",
                    target, constraint
                )
            } else {
                format!(
                    "Root composer.json requires {} {}, but no matching package was found",
                    target, constraint
                )
            }
        }
        RuleType::Fixed => {
            let source = get_source_name(rule, pool);
            format!("{} is fixed and cannot be changed", source)
        }
        RuleType::PackageRequires => {
            let source = get_source_name(rule, pool);
            let target = rule.target.as_deref().unwrap_or("unknown");
            let constraint = rule.constraint.as_deref().unwrap_or("*");
            format!("{} requires {} {}", source, target, constraint)
        }
        RuleType::PackageConflict => {
            let source = get_source_name(rule, pool);
            let target = rule.target.as_deref().unwrap_or("unknown");
            format!("{} conflicts with {}", source, target)
        }
        RuleType::PackageSameName | RuleType::MultiConflict => {
            "Only one version of a package can be installed".to_string()
        }
        RuleType::PackageAlias | RuleType::PackageInverseAlias => {
            "Package alias constraint".to_string()
        }
        RuleType::Learned => {
            "Learned constraint from conflict analysis".to_string()
        }
    }
}

/// Collection of problems encountered during solving
#[derive(Debug, Default)]
pub struct ProblemSet {
    problems: Vec<Problem>,
}

impl ProblemSet {
    /// Create a new empty problem set
    pub fn new() -> Self {
        Self {
            problems: Vec::new(),
        }
    }

    /// Add a problem
    pub fn add(&mut self, problem: Problem) {
        self.problems.push(problem);
    }

    /// Check if there are any problems
    pub fn is_empty(&self) -> bool {
        self.problems.is_empty()
    }

    /// Get the number of problems
    pub fn len(&self) -> usize {
        self.problems.len()
    }

    /// Get all problems
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// Generate a complete description of all problems
    pub fn describe(&self, pool: &Pool) -> String {
        let descriptions: Vec<_> = self.problems
            .iter()
            .enumerate()
            .map(|(i, p)| format!("Problem {}:\n{}", i + 1, p.describe(pool)))
            .collect();

        if descriptions.is_empty() {
            "No problems found".to_string()
        } else {
            descriptions.join("\n\n")
        }
    }
}

impl fmt::Display for ProblemSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} problem(s) found", self.problems.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_problem_new() {
        let problem = Problem::new();
        assert!(problem.rules.is_empty());
    }

    #[test]
    fn test_problem_add_rule() {
        let mut problem = Problem::new();
        let rule = Rule::root_require(vec![1, 2])
            .with_target("vendor/package")
            .with_constraint("^1.0");

        problem.add_rule(&rule);
        assert_eq!(problem.rules.len(), 1);
    }

    #[test]
    fn test_problem_describe() {
        let pool = Pool::new();
        let mut problem = Problem::new();

        let rule = Rule::root_require(vec![])
            .with_target("vendor/package")
            .with_constraint("^1.0");
        problem.add_rule(&rule);

        let description = problem.describe(&pool);
        assert!(description.contains("vendor/package"));
        assert!(description.contains("^1.0"));
    }

    #[test]
    fn test_problem_set() {
        let mut problems = ProblemSet::new();
        assert!(problems.is_empty());

        problems.add(Problem::new());
        assert_eq!(problems.len(), 1);
        assert!(!problems.is_empty());
    }
}
