//! Version comparison utilities

use crate::constraint::constraint::php_version_compare;

/// Comparator for comparing version strings
pub struct Comparator;

impl Comparator {
    /// Check if version1 > version2
    pub fn greater_than(version1: &str, version2: &str) -> bool {
        Self::compare(version1, ">", version2)
    }

    /// Check if version1 >= version2
    pub fn greater_than_or_equal_to(version1: &str, version2: &str) -> bool {
        Self::compare(version1, ">=", version2)
    }

    /// Check if version1 < version2
    pub fn less_than(version1: &str, version2: &str) -> bool {
        Self::compare(version1, "<", version2)
    }

    /// Check if version1 <= version2
    pub fn less_than_or_equal_to(version1: &str, version2: &str) -> bool {
        Self::compare(version1, "<=", version2)
    }

    /// Check if version1 == version2
    pub fn equal_to(version1: &str, version2: &str) -> bool {
        Self::compare(version1, "==", version2)
    }

    /// Check if version1 != version2
    pub fn not_equal_to(version1: &str, version2: &str) -> bool {
        Self::compare(version1, "!=", version2)
    }

    /// Compare version1 to version2 using the given operator
    pub fn compare(version1: &str, operator: &str, version2: &str) -> bool {
        // Handle dev branches specially
        let v1_is_branch = version1.starts_with("dev-");
        let v2_is_branch = version2.starts_with("dev-");

        // For != operator with branches
        if (operator == "!=" || operator == "<>") && (v1_is_branch || v2_is_branch) {
            return version1 != version2;
        }

        // Two branches can only be == if identical
        if v1_is_branch && v2_is_branch {
            return operator == "==" && version1 == version2;
        }

        // When one is a branch and one is not, the version is always "greater"
        if v1_is_branch && !v2_is_branch {
            return matches!(operator, "<" | "<=" | "!=" | "<>");
        }
        if !v1_is_branch && v2_is_branch {
            return matches!(operator, ">" | ">=" | "!=" | "<>");
        }

        php_version_compare(version1, version2, operator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_greater_than() {
        assert!(Comparator::greater_than("1.25.0", "1.24.0"));
        assert!(!Comparator::greater_than("1.25.0", "1.25.0"));
        assert!(!Comparator::greater_than("1.25.0", "1.26.0"));
        assert!(Comparator::greater_than("1.26.0", "dev-foo"));
        assert!(!Comparator::greater_than("dev-foo", "dev-master"));
        assert!(!Comparator::greater_than("dev-foo", "dev-bar"));
    }

    #[test]
    fn test_greater_than_or_equal_to() {
        assert!(Comparator::greater_than_or_equal_to("1.25.0", "1.24.0"));
        assert!(Comparator::greater_than_or_equal_to("1.25.0", "1.25.0"));
        assert!(!Comparator::greater_than_or_equal_to("1.25.0", "1.26.0"));
    }

    #[test]
    fn test_less_than() {
        assert!(!Comparator::less_than("1.25.0", "1.24.0"));
        assert!(!Comparator::less_than("1.25.0", "1.25.0"));
        assert!(Comparator::less_than("1.25.0", "1.26.0"));
        assert!(Comparator::less_than("1.0.0", "1.2-dev"));
        assert!(Comparator::less_than("dev-foo", "1.26.0"));
        assert!(!Comparator::less_than("dev-foo", "dev-master"));
        assert!(!Comparator::less_than("dev-foo", "dev-bar"));
    }

    #[test]
    fn test_less_than_or_equal_to() {
        assert!(!Comparator::less_than_or_equal_to("1.25.0", "1.24.0"));
        assert!(Comparator::less_than_or_equal_to("1.25.0", "1.25.0"));
        assert!(Comparator::less_than_or_equal_to("1.25.0", "1.26.0"));
    }

    #[test]
    fn test_equal_to() {
        assert!(!Comparator::equal_to("1.25.0", "1.24.0"));
        assert!(Comparator::equal_to("1.25.0", "1.25.0"));
        assert!(!Comparator::equal_to("1.25.0", "1.26.0"));
        assert!(!Comparator::equal_to("dev-foo", "1.26.0"));
        assert!(!Comparator::equal_to("dev-foo", "dev-master"));
        assert!(!Comparator::equal_to("dev-foo", "dev-bar"));
    }

    #[test]
    fn test_not_equal_to() {
        assert!(Comparator::not_equal_to("1.25.0", "1.24.0"));
        assert!(!Comparator::not_equal_to("1.25.0", "1.25.0"));
        assert!(Comparator::not_equal_to("1.25.0", "1.26.0"));
    }

    #[test]
    fn test_compare() {
        // Greater than
        assert!(Comparator::compare("1.25.0", ">", "1.24.0"));
        assert!(!Comparator::compare("1.25.0", ">", "1.25.0"));
        assert!(!Comparator::compare("1.25.0", ">", "1.26.0"));

        // Greater than or equal
        assert!(Comparator::compare("1.25.0", ">=", "1.24.0"));
        assert!(Comparator::compare("1.25.0", ">=", "1.25.0"));
        assert!(!Comparator::compare("1.25.0", ">=", "1.26.0"));

        // Less than
        assert!(!Comparator::compare("1.25.0", "<", "1.24.0"));
        assert!(!Comparator::compare("1.25.0", "<", "1.25.0"));
        assert!(Comparator::compare("1.25.0", "<", "1.26.0"));

        // Less than or equal
        assert!(!Comparator::compare("1.25.0", "<=", "1.24.0"));
        assert!(Comparator::compare("1.25.0", "<=", "1.25.0"));
        assert!(Comparator::compare("1.25.0", "<=", "1.26.0"));

        // Equal
        assert!(!Comparator::compare("1.25.0", "==", "1.24.0"));
        assert!(Comparator::compare("1.25.0", "==", "1.25.0"));
        assert!(!Comparator::compare("1.25.0", "==", "1.26.0"));

        // Equal with = alias
        assert!(!Comparator::compare("1.25.0", "=", "1.24.0"));
        assert!(Comparator::compare("1.25.0", "=", "1.25.0"));
        assert!(!Comparator::compare("1.25.0", "=", "1.26.0"));

        // Not equal
        assert!(Comparator::compare("1.25.0", "!=", "1.24.0"));
        assert!(!Comparator::compare("1.25.0", "!=", "1.25.0"));
        assert!(Comparator::compare("1.25.0", "!=", "1.26.0"));

        // Not equal with <> alias
        assert!(Comparator::compare("1.25.0", "<>", "1.24.0"));
        assert!(!Comparator::compare("1.25.0", "<>", "1.25.0"));
        assert!(Comparator::compare("1.25.0", "<>", "1.26.0"));
    }

    #[test]
    fn test_compare_with_stability() {
        assert!(Comparator::compare("1.25.0-beta2.1", "<", "1.25.0-b.3"));
        assert!(Comparator::compare("1.25.0-b2.1", "<", "1.25.0beta.3"));
        assert!(Comparator::compare("1.25.0-b-2.1", "<", "1.25.0-rc"));
        assert!(Comparator::compare("1.25.0-beta2.1", "==", "1.25.0-b.2.1"));
        assert!(Comparator::compare("1.25.0beta2.1", "==", "1.25.0-b2.1"));
    }
}
