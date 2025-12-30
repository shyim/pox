//! Semver facade providing high-level version operations

use crate::{Comparator, ParsedConstraints, VersionParser};
use crate::constraint::{Constraint, Operator};

/// Main facade for semantic versioning operations
pub struct Semver;

impl Semver {
    /// Check if a version satisfies a constraint
    pub fn satisfies(version: &str, constraints: &str) -> bool {
        let parser = VersionParser::new();

        // Normalize the version
        let normalized = match parser.normalize(version) {
            Ok(v) => v,
            Err(_) => return false,
        };

        // Parse the constraints
        let parsed_constraints = match parser.parse_constraints(constraints) {
            Ok(c) => c,
            Err(_) => return false,
        };

        // Create a provider constraint for the version
        let provider = match Constraint::new(Operator::Equal, normalized) {
            Ok(c) => c,
            Err(_) => return false,
        };

        // Check if the constraints match the provider
        parsed_constraints.matches(&provider)
    }

    /// Return all versions that satisfy the given constraints
    pub fn satisfied_by(versions: &[&str], constraints: &str) -> Vec<String> {
        let parser = VersionParser::new();
        let parsed_constraints = match parser.parse_constraints_cached(constraints) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        versions
            .iter()
            .filter_map(|v| {
                let normalized = parser.normalize(v).ok()?;
                if parsed_constraints.matches_normalized(&normalized) {
                    Some(v.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Parse constraints and return a reusable representation.
    pub fn parse_constraints(constraints: &str) -> Result<ParsedConstraints, crate::VersionParserError> {
        let parser = VersionParser::new();
        parser.parse_constraints_cached(constraints)
    }

    /// Check a version against pre-parsed constraints.
    pub fn satisfies_parsed(version: &str, constraints: &ParsedConstraints) -> bool {
        constraints.satisfies(version)
    }

    /// Sort versions in ascending order
    pub fn sort(versions: &[&str]) -> Vec<String> {
        Self::usort(versions, true)
    }

    /// Sort versions in descending order (reverse sort)
    pub fn rsort(versions: &[&str]) -> Vec<String> {
        Self::usort(versions, false)
    }

    fn usort(versions: &[&str], ascending: bool) -> Vec<String> {
        let parser = VersionParser::new();

        // Create normalized versions with their original index
        let mut normalized: Vec<(String, usize)> = versions
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let norm = parser.normalize(v).ok()?;
                let norm = parser.normalize_default_branch(&norm);
                Some((norm, i))
            })
            .collect();

        // Sort by normalized version
        normalized.sort_by(|(a, _), (b, _)| {
            let cmp = if Comparator::less_than(a, b) {
                std::cmp::Ordering::Less
            } else if Comparator::equal_to(a, b) {
                std::cmp::Ordering::Equal
            } else {
                std::cmp::Ordering::Greater
            };

            if ascending {
                cmp
            } else {
                cmp.reverse()
            }
        });

        // Return original versions in sorted order
        normalized
            .into_iter()
            .map(|(_, i)| versions[i].to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_satisfies_positive() {
        // Full test suite from PHP satisfiesProviderPositive

        // Hyphen ranges
        assert!(Semver::satisfies("1.2.3", "1.0.0 - 2.0.0"));
        assert!(Semver::satisfies("1.2.3", "1.2.3+asdf - 2.4.3+asdf"));
        assert!(Semver::satisfies("2.4.3-alpha", "1.2.3+asdf - 2.4.3+asdf"));

        // Caret with build metadata
        assert!(Semver::satisfies("1.2.3", "^1.2.3+build"));
        assert!(Semver::satisfies("1.3.0", "^1.2.3+build"));

        // Prerelease with operators
        assert!(Semver::satisfies("1.3.0-beta", ">1.2"));
        assert!(Semver::satisfies("1.2.3-beta", "<=1.2.3"));
        assert!(Semver::satisfies("1.2.3-beta", "^1.2.3"));

        // Basic constraints
        assert!(Semver::satisfies("1.0.0", "1.0.0"));
        assert!(Semver::satisfies("1.2.3", "*"));
        assert!(Semver::satisfies("v1.2.3", "*"));

        // Greater than/less than
        assert!(Semver::satisfies("1.0.0", ">=1.0.0"));
        assert!(Semver::satisfies("1.0.1", ">=1.0.0"));
        assert!(Semver::satisfies("1.1.0", ">=1.0.0"));
        assert!(Semver::satisfies("1.0.1", ">1.0.0"));
        assert!(Semver::satisfies("1.1.0", ">1.0.0"));
        assert!(Semver::satisfies("2.0.0", "<=2.0.0"));
        assert!(Semver::satisfies("1.9999.9999", "<=2.0.0"));
        assert!(Semver::satisfies("0.2.9", "<=2.0.0"));
        assert!(Semver::satisfies("1.9999.9999", "<2.0.0"));
        assert!(Semver::satisfies("0.2.9", "<2.0.0"));

        // With spaces
        assert!(Semver::satisfies("1.0.0", ">= 1.0.0"));
        assert!(Semver::satisfies("1.0.1", ">=  1.0.0"));
        assert!(Semver::satisfies("1.1.0", ">=   1.0.0"));
        assert!(Semver::satisfies("1.0.1", "> 1.0.0"));
        assert!(Semver::satisfies("1.1.0", ">  1.0.0"));
        assert!(Semver::satisfies("2.0.0", "<=   2.0.0"));
        assert!(Semver::satisfies("1.9999.9999", "<= 2.0.0"));
        assert!(Semver::satisfies("0.2.9", "<=  2.0.0"));
        assert!(Semver::satisfies("1.9999.9999", "<    2.0.0"));

        // Version with v prefix
        assert!(Semver::satisfies("v0.1.97", ">=0.1.97"));
        assert!(Semver::satisfies("0.1.97", ">=0.1.97"));

        // Or constraints
        assert!(Semver::satisfies("1.2.4", "0.1.20 || 1.2.4"));
        assert!(Semver::satisfies("0.0.0", ">=0.2.3 || <0.0.1"));
        assert!(Semver::satisfies("0.2.3", ">=0.2.3 || <0.0.1"));
        assert!(Semver::satisfies("0.2.4", ">=0.2.3 || <0.0.1"));

        // Wildcard
        assert!(Semver::satisfies("2.1.3", "2.x.x"));
        assert!(Semver::satisfies("1.2.3", "1.2.x"));
        assert!(Semver::satisfies("2.1.3", "1.2.x || 2.x"));
        assert!(Semver::satisfies("1.2.3", "1.2.x || 2.x"));
        assert!(Semver::satisfies("1.2.3", "x"));
        assert!(Semver::satisfies("2.1.3", "2.*.*"));
        assert!(Semver::satisfies("1.2.3", "1.2.*"));
        assert!(Semver::satisfies("2.1.3", "1.2.* || 2.*"));
        assert!(Semver::satisfies("1.2.3", "1.2.* || 2.*"));
        assert!(Semver::satisfies("1.2.3", "*"));

        // Tilde
        assert!(Semver::satisfies("2.9.0", "~2.4"));
        assert!(Semver::satisfies("2.4.5", "~2.4"));
        assert!(Semver::satisfies("1.2.3", "~1"));
        assert!(Semver::satisfies("1.4.7", "~1.0"));

        // Simple version checks
        assert!(Semver::satisfies("1.0.0", ">=1"));
        assert!(Semver::satisfies("1.0.0", ">= 1"));
        assert!(Semver::satisfies("1.2.8", ">1.2"));
        assert!(Semver::satisfies("1.1.1", "<1.2"));
        assert!(Semver::satisfies("1.1.1", "< 1.2"));

        // Combined constraints
        assert!(Semver::satisfies("1.2.3", "~1.2.1 >=1.2.3"));
        assert!(Semver::satisfies("1.2.3", "~1.2.1 =1.2.3"));
        assert!(Semver::satisfies("1.2.3", "~1.2.1 1.2.3"));
        assert!(Semver::satisfies("1.2.3", "~1.2.1 >=1.2.3 1.2.3"));
        assert!(Semver::satisfies("1.2.3", "~1.2.1 1.2.3 >=1.2.3"));
        assert!(Semver::satisfies("1.2.3", "~1.2.1 1.2.3"));
        assert!(Semver::satisfies("1.2.3", ">=1.2.1 1.2.3"));
        assert!(Semver::satisfies("1.2.3", "1.2.3 >=1.2.1"));
        assert!(Semver::satisfies("1.2.3", ">=1.2.3 >=1.2.1"));
        assert!(Semver::satisfies("1.2.3", ">=1.2.1 >=1.2.3"));
        assert!(Semver::satisfies("1.2.8", ">=1.2"));

        // Caret
        assert!(Semver::satisfies("1.8.1", "^1.2.3"));
        assert!(Semver::satisfies("0.1.2", "^0.1.2"));
        assert!(Semver::satisfies("0.1.2", "^0.1"));
        assert!(Semver::satisfies("1.4.2", "^1.2"));
        assert!(Semver::satisfies("1.4.2", "^1.2 ^1"));
        assert!(Semver::satisfies("0.0.1-beta", "^0.0.1-alpha"));
    }

    #[test]
    fn test_satisfies_negative() {
        // Full test suite from PHP satisfiesProviderNegative

        // Hyphen ranges
        assert!(!Semver::satisfies("2.2.3", "1.0.0 - 2.0.0"));

        // Caret with build metadata
        assert!(!Semver::satisfies("2.0.0", "^1.2.3+build"));
        assert!(!Semver::satisfies("1.2.0", "^1.2.3+build"));

        // Beta version against exact
        assert!(!Semver::satisfies("1.0.0beta", "1"));
        assert!(!Semver::satisfies("1.0.0beta", "<1"));
        assert!(!Semver::satisfies("1.0.0beta", "< 1"));

        // Exact version mismatch
        assert!(!Semver::satisfies("1.0.1", "1.0.0"));

        // Greater than/less than failures
        assert!(!Semver::satisfies("0.0.0", ">=1.0.0"));
        assert!(!Semver::satisfies("0.0.1", ">=1.0.0"));
        assert!(!Semver::satisfies("0.1.0", ">=1.0.0"));
        assert!(!Semver::satisfies("0.0.1", ">1.0.0"));
        assert!(!Semver::satisfies("0.1.0", ">1.0.0"));
        assert!(!Semver::satisfies("3.0.0", "<=2.0.0"));
        assert!(!Semver::satisfies("2.9999.9999", "<=2.0.0"));
        assert!(!Semver::satisfies("2.2.9", "<=2.0.0"));
        assert!(!Semver::satisfies("2.9999.9999", "<2.0.0"));
        assert!(!Semver::satisfies("2.2.9", "<2.0.0"));

        // Version with v prefix
        assert!(!Semver::satisfies("v0.1.93", ">=0.1.97"));
        assert!(!Semver::satisfies("0.1.93", ">=0.1.97"));

        // Or constraints
        assert!(!Semver::satisfies("1.2.3", "0.1.20 || 1.2.4"));
        assert!(!Semver::satisfies("0.0.3", ">=0.2.3 || <0.0.1"));
        assert!(!Semver::satisfies("0.2.2", ">=0.2.3 || <0.0.1"));

        // Wildcard
        assert!(!Semver::satisfies("1.1.3", "2.x.x"));
        assert!(!Semver::satisfies("3.1.3", "2.x.x"));
        assert!(!Semver::satisfies("1.3.3", "1.2.x"));
        assert!(!Semver::satisfies("3.1.3", "1.2.x || 2.x"));
        assert!(!Semver::satisfies("1.1.3", "1.2.x || 2.x"));
        assert!(!Semver::satisfies("1.1.3", "2.*.*"));
        assert!(!Semver::satisfies("3.1.3", "2.*.*"));
        assert!(!Semver::satisfies("1.3.3", "1.2.*"));
        assert!(!Semver::satisfies("3.1.3", "1.2.* || 2.*"));
        assert!(!Semver::satisfies("1.1.3", "1.2.* || 2.*"));

        // Exact major/minor mismatch
        assert!(!Semver::satisfies("1.1.2", "2"));
        assert!(!Semver::satisfies("2.4.1", "2.3"));

        // Tilde
        assert!(!Semver::satisfies("3.0.0", "~2.4"));
        assert!(!Semver::satisfies("2.3.9", "~2.4"));
        assert!(!Semver::satisfies("0.2.3", "~1"));

        // Less than
        assert!(!Semver::satisfies("1.0.0", "<1"));
        assert!(!Semver::satisfies("1.1.1", ">=1.2"));

        // Beta versions
        assert!(!Semver::satisfies("2.0.0beta", "1"));
        assert!(!Semver::satisfies("0.5.4-alpha", "~v0.5.4-beta"));

        // Prerelease with operators
        assert!(!Semver::satisfies("1.2.3-beta", "<1.2.3"));
        assert!(!Semver::satisfies("2.0.0-alpha", "^1.2.3"));

        // Caret
        assert!(!Semver::satisfies("1.2.2", "^1.2.3"));
        assert!(!Semver::satisfies("1.1.9", "^1.2"));
    }

    #[test]
    fn test_satisfied_by() {
        let versions = vec!["1.0", "1.2", "1.9999.9999", "2.0", "2.1", "0.9999.9999"];
        let result = Semver::satisfied_by(&versions, "~1.0");
        assert_eq!(result, vec!["1.0", "1.2", "1.9999.9999"]);

        // Complex constraint parsing with AND
        let versions2 = vec!["1.0", "1.1", "2.9999.9999", "3.0", "3.1", "3.9999.9999", "4.0", "4.1"];
        let result2 = Semver::satisfied_by(&versions2, ">1.0 <3.0 || >=4.0");
        assert_eq!(result2, vec!["1.1", "2.9999.9999", "4.0", "4.1"]);

        let versions3 = vec!["0.1.1", "0.1.9999", "0.2.0", "0.2.1", "0.3.0"];
        let result3 = Semver::satisfied_by(&versions3, "^0.2.0");
        assert_eq!(result3, vec!["0.2.0", "0.2.1"]);
    }

    #[test]
    fn test_sort() {
        let versions = vec!["1.0", "0.1", "0.1", "3.2.1", "2.4.0-alpha", "2.4.0"];
        let sorted = Semver::sort(&versions);
        assert_eq!(sorted, vec!["0.1", "0.1", "1.0", "2.4.0-alpha", "2.4.0", "3.2.1"]);

        let versions2 = vec!["dev-foo", "dev-master", "1.0", "50.2"];
        let sorted2 = Semver::sort(&versions2);
        assert_eq!(sorted2, vec!["dev-foo", "1.0", "50.2", "dev-master"]);
    }

    #[test]
    fn test_rsort() {
        let versions = vec!["1.0", "0.1", "0.1", "3.2.1", "2.4.0-alpha", "2.4.0"];
        let rsorted = Semver::rsort(&versions);
        assert_eq!(rsorted, vec!["3.2.1", "2.4.0", "2.4.0-alpha", "1.0", "0.1", "0.1"]);

        let versions2 = vec!["dev-foo", "dev-master", "1.0", "50.2"];
        let rsorted2 = Semver::rsort(&versions2);
        assert_eq!(rsorted2, vec!["dev-master", "50.2", "1.0", "dev-foo"]);
    }

    #[test]
    fn test_parsed_constraints_reuse() {
        let parsed = Semver::parse_constraints("^1.2").unwrap();
        assert!(Semver::satisfies_parsed("1.2.3", &parsed));
        assert!(Semver::satisfies_parsed("1.9.0", &parsed));
        assert!(!Semver::satisfies_parsed("2.0.0", &parsed));
    }
}
