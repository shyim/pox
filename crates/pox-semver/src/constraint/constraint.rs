//! Single version constraint implementation

use std::fmt;
use thiserror::Error;

use super::{Bound, ConstraintInterface, Operator};

#[derive(Error, Debug)]
pub enum ConstraintError {
    #[error("Invalid operator \"{operator}\", expected one of: {expected}")]
    InvalidOperator { operator: String, expected: String },
}

/// A single version constraint (e.g., ">= 1.0.0")
#[derive(Debug, Clone)]
pub struct Constraint {
    operator: Operator,
    version: String,
    pretty_string: Option<String>,
    lower_bound: Option<Bound>,
    upper_bound: Option<Bound>,
}

impl Constraint {
    /// Create a new constraint
    pub fn new(operator: Operator, version: String) -> Result<Self, ConstraintError> {
        Ok(Constraint {
            operator,
            version,
            pretty_string: None,
            lower_bound: None,
            upper_bound: None,
        })
    }

    /// Create a constraint from operator string
    pub fn from_str(operator: &str, version: String) -> Result<Self, ConstraintError> {
        let op = Operator::from_str(operator).map_err(|_| ConstraintError::InvalidOperator {
            operator: operator.to_string(),
            expected: Operator::supported_operators().join(", "),
        })?;
        Self::new(op, version)
    }

    /// Get the version
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Get the operator
    pub fn operator(&self) -> Operator {
        self.operator
    }

    /// Match against another single constraint
    pub fn match_specific(&self, provider: &Constraint, compare_branches: bool) -> bool {
        let is_equal_op = self.operator == Operator::Equal;
        let is_non_equal_op = self.operator == Operator::NotEqual;
        let is_provider_equal_op = provider.operator == Operator::Equal;
        let is_provider_non_equal_op = provider.operator == Operator::NotEqual;

        // != operator handling
        if is_non_equal_op || is_provider_non_equal_op {
            if is_non_equal_op
                && !is_provider_non_equal_op
                && !is_provider_equal_op
                && provider.version.starts_with("dev-")
            {
                return false;
            }

            if is_provider_non_equal_op
                && !is_non_equal_op
                && !is_equal_op
                && self.version.starts_with("dev-")
            {
                return false;
            }

            if !is_equal_op && !is_provider_equal_op {
                return true;
            }

            return self.version_compare(&provider.version, &self.version, Operator::NotEqual, compare_branches);
        }

        // Same direction comparisons always have a solution (both < or both >)
        // Check if both operators are in the same "direction" (both less-than-ish or both greater-than-ish)
        let self_direction = match self.operator {
            Operator::LessThan | Operator::LessThanOrEqual => Some("less"),
            Operator::GreaterThan | Operator::GreaterThanOrEqual => Some("greater"),
            _ => None,
        };
        let provider_direction = match provider.operator {
            Operator::LessThan | Operator::LessThanOrEqual => Some("less"),
            Operator::GreaterThan | Operator::GreaterThanOrEqual => Some("greater"),
            _ => None,
        };

        if self_direction.is_some() && self_direction == provider_direction {
            return !(self.version.starts_with("dev-") || provider.version.starts_with("dev-"));
        }

        let (version1, version2, operator) = if is_equal_op {
            (&self.version, &provider.version, provider.operator)
        } else {
            (&provider.version, &self.version, self.operator)
        };

        if self.version_compare(version1, version2, operator, compare_branches) {
            // Special case: opposite direction operators with no intersection
            // e.g., require >= 1.0 and provide < 1.0 should NOT match
            // But require >= 2 and provide <= 2 SHOULD match (they meet at 2)
            if !is_equal_op && !is_provider_equal_op {
                // Check if operators are opposite directions
                let opposite_directions = self_direction.is_some()
                    && provider_direction.is_some()
                    && self_direction != provider_direction;

                if opposite_directions {
                    // If same version but opposite directions, check if they can meet
                    if php_version_compare(&provider.version, &self.version, "==") {
                        // Same version - they only intersect if both are inclusive
                        let self_inclusive = self.operator == Operator::LessThanOrEqual
                            || self.operator == Operator::GreaterThanOrEqual;
                        let provider_inclusive = provider.operator == Operator::LessThanOrEqual
                            || provider.operator == Operator::GreaterThanOrEqual;
                        return self_inclusive && provider_inclusive;
                    }
                    // Different versions - opposite directions always intersect somewhere
                    return true;
                }
            }
            return true;
        }

        false
    }

    /// Compare two versions with an operator
    pub fn version_compare(
        &self,
        a: &str,
        b: &str,
        operator: Operator,
        compare_branches: bool,
    ) -> bool {
        let a_is_branch = a.starts_with("dev-");
        let b_is_branch = b.starts_with("dev-");

        if operator == Operator::NotEqual && (a_is_branch || b_is_branch) {
            return a != b;
        }

        if a_is_branch && b_is_branch {
            return operator == Operator::Equal && a == b;
        }

        // When branches are not comparable, dev branches never match anything
        if !compare_branches && (a_is_branch || b_is_branch) {
            return false;
        }

        php_version_compare(a, b, operator.as_str())
    }

    fn extract_bounds(&mut self) {
        if self.lower_bound.is_some() {
            return;
        }

        // Branches have infinite bounds
        if self.version.starts_with("dev-") {
            self.lower_bound = Some(Bound::zero());
            self.upper_bound = Some(Bound::positive_infinity());
            return;
        }

        match self.operator {
            Operator::Equal => {
                self.lower_bound = Some(Bound::new(self.version.clone(), true));
                self.upper_bound = Some(Bound::new(self.version.clone(), true));
            }
            Operator::LessThan => {
                self.lower_bound = Some(Bound::zero());
                self.upper_bound = Some(Bound::new(self.version.clone(), false));
            }
            Operator::LessThanOrEqual => {
                self.lower_bound = Some(Bound::zero());
                self.upper_bound = Some(Bound::new(self.version.clone(), true));
            }
            Operator::GreaterThan => {
                self.lower_bound = Some(Bound::new(self.version.clone(), false));
                self.upper_bound = Some(Bound::positive_infinity());
            }
            Operator::GreaterThanOrEqual => {
                self.lower_bound = Some(Bound::new(self.version.clone(), true));
                self.upper_bound = Some(Bound::positive_infinity());
            }
            Operator::NotEqual => {
                self.lower_bound = Some(Bound::zero());
                self.upper_bound = Some(Bound::positive_infinity());
            }
        }
    }
}

impl ConstraintInterface for Constraint {
    fn matches(&self, other: &dyn ConstraintInterface) -> bool {
        // If other is a single Constraint, use match_specific
        if let Some((op, ver)) = other.as_constraint() {
            if let Ok(provider) = Constraint::new(*op, ver.to_string()) {
                return self.match_specific(&provider, false);
            }
        }

        // If other is MatchAllConstraint
        if other.is_match_all() {
            return true;
        }

        // If other is MatchNoneConstraint
        if other.is_match_none() {
            return false;
        }

        // For MultiConstraint, delegate to its matches
        other.matches(self)
    }

    fn lower_bound(&self) -> Bound {
        let mut s = self.clone();
        s.extract_bounds();
        s.lower_bound.unwrap()
    }

    fn upper_bound(&self) -> Bound {
        let mut s = self.clone();
        s.extract_bounds();
        s.upper_bound.unwrap()
    }

    fn pretty_string(&self) -> String {
        self.pretty_string
            .clone()
            .unwrap_or_else(|| self.to_string())
    }

    fn set_pretty_string(&mut self, pretty: Option<String>) {
        self.pretty_string = pretty;
    }

    fn clone_box(&self) -> Box<dyn ConstraintInterface> {
        Box::new(self.clone())
    }

    fn as_constraint(&self) -> Option<(&Operator, &str)> {
        Some((&self.operator, &self.version))
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.operator, self.version)
    }
}

/// PHP-compatible version_compare
pub fn php_version_compare(a: &str, b: &str, operator: &str) -> bool {
    let cmp = compare_versions(a, b);

    match operator {
        "==" | "=" => cmp == std::cmp::Ordering::Equal,
        "!=" | "<>" => cmp != std::cmp::Ordering::Equal,
        "<" => cmp == std::cmp::Ordering::Less,
        "<=" => cmp != std::cmp::Ordering::Greater,
        ">" => cmp == std::cmp::Ordering::Greater,
        ">=" => cmp != std::cmp::Ordering::Less,
        _ => false,
    }
}

/// Compare two version strings (PHP version_compare compatible)
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_iter = PartIter::new(a);
    let mut b_iter = PartIter::new(b);

    loop {
        let a_part = a_iter.next();
        let b_part = b_iter.next();

        if a_part.is_none() && b_part.is_none() {
            return std::cmp::Ordering::Equal;
        }

        let a_part = match a_part {
            Some(part) => part,
            None => Part::empty(),
        };
        let b_part = match b_part {
            Some(part) => part,
            None => Part::empty(),
        };

        let cmp = compare_part(a_part, b_part);
        if cmp != std::cmp::Ordering::Equal {
            return cmp;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Digit,
    Alpha,
}

#[derive(Clone, Copy)]
struct Part<'a> {
    kind: PartKind,
    text: &'a str,
}

impl<'a> Part<'a> {
    fn empty() -> Part<'static> {
        Part {
            kind: PartKind::Alpha,
            text: "",
        }
    }
}

struct PartIter<'a> {
    input: &'a str,
    bytes: &'a [u8],
    index: usize,
}

impl<'a> PartIter<'a> {
    fn new(input: &'a str) -> Self {
        PartIter {
            input,
            bytes: input.as_bytes(),
            index: 0,
        }
    }

    fn next(&mut self) -> Option<Part<'a>> {
        let len = self.bytes.len();
        while self.index < len && !self.bytes[self.index].is_ascii_alphanumeric() {
            self.index += 1;
        }
        if self.index >= len {
            return None;
        }

        let start = self.index;
        let is_digit = self.bytes[self.index].is_ascii_digit();
        self.index += 1;

        while self.index < len {
            let b = self.bytes[self.index];
            if is_digit {
                if b.is_ascii_digit() {
                    self.index += 1;
                    continue;
                }
            } else if b.is_ascii_alphabetic() {
                self.index += 1;
                continue;
            }
            break;
        }

        Some(Part {
            kind: if is_digit { PartKind::Digit } else { PartKind::Alpha },
            text: &self.input[start..self.index],
        })
    }
}

fn compare_part(a: Part<'_>, b: Part<'_>) -> std::cmp::Ordering {
    let a_num = if a.kind == PartKind::Digit {
        parse_i64_ascii(a.text)
    } else {
        None
    };
    let b_num = if b.kind == PartKind::Digit {
        parse_i64_ascii(b.text)
    } else {
        None
    };

    match (a_num, b_num) {
        (Some(an), Some(bn)) => an.cmp(&bn),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => {
            let a_order = special_order(a.text);
            let b_order = special_order(b.text);
            a_order.cmp(&b_order)
        }
    }
}

fn parse_i64_ascii(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let mut value: i64 = 0;
    for b in s.as_bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        let digit = (b - b'0') as i64;
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

fn special_order(s: &str) -> i32 {
    if s.is_empty() || s.eq_ignore_ascii_case("stable") {
        return 4;
    }
    if s.eq_ignore_ascii_case("dev") {
        return 0;
    }
    if s.eq_ignore_ascii_case("alpha") || s.eq_ignore_ascii_case("a") {
        return 1;
    }
    if s.eq_ignore_ascii_case("beta") || s.eq_ignore_ascii_case("b") {
        return 2;
    }
    if s.eq_ignore_ascii_case("rc") {
        return 3;
    }
    if s.eq_ignore_ascii_case("patch") || s.eq_ignore_ascii_case("pl") || s.eq_ignore_ascii_case("p") {
        return 5;
    }
    4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constraint_creation() {
        let c = Constraint::new(Operator::Equal, "1.0.0".to_string()).unwrap();
        assert_eq!(c.version(), "1.0.0");
        assert_eq!(c.operator(), Operator::Equal);
    }

    #[test]
    fn test_constraint_display() {
        let c = Constraint::new(Operator::GreaterThanOrEqual, "1.0.0".to_string()).unwrap();
        assert_eq!(c.to_string(), ">= 1.0.0");
    }

    #[test]
    fn test_version_compare() {
        assert!(php_version_compare("1.0.0", "1.0.0", "=="));
        assert!(php_version_compare("2.0.0", "1.0.0", ">"));
        assert!(php_version_compare("1.0.0", "2.0.0", "<"));
        assert!(php_version_compare("1.0.0", "1.0.0", ">="));
        assert!(php_version_compare("1.0.0", "1.0.0", "<="));
        assert!(!php_version_compare("1.0.0", "1.0.0", "!="));
    }

    #[test]
    fn test_dev_version_stability() {
        // Dev versions should be less than stable versions
        // 2.1.0.0-dev < 2.1.0.0
        assert!(!php_version_compare("2.1.0.0-dev", "2.1.0.0", ">="), "2.1.0.0-dev should NOT be >= 2.1.0.0");
        assert!(php_version_compare("2.1.0.0-dev", "2.1.0.0", "<"), "2.1.0.0-dev should be < 2.1.0.0");

        // 2.1-dev normalized is typically 2.1.0.0-dev
        assert!(!php_version_compare("2.1-dev", "2.1", ">="), "2.1-dev should NOT be >= 2.1");
        assert!(php_version_compare("2.1-dev", "2.1", "<"), "2.1-dev should be < 2.1");
    }

    #[test]
    fn test_match_specific() {
        let c1 = Constraint::new(Operator::GreaterThan, "1.0.0".to_string()).unwrap();
        let c2 = Constraint::new(Operator::Equal, "2.0.0".to_string()).unwrap();
        assert!(c1.match_specific(&c2, false));

        let c3 = Constraint::new(Operator::Equal, "0.5.0".to_string()).unwrap();
        assert!(!c1.match_specific(&c3, false));
    }

    #[test]
    fn test_bounds() {
        let c = Constraint::new(Operator::GreaterThanOrEqual, "1.0.0".to_string()).unwrap();
        assert_eq!(c.lower_bound().version(), "1.0.0");
        assert!(c.lower_bound().is_inclusive());
        assert!(c.upper_bound().is_positive_infinity());
    }

    #[test]
    fn test_equal_equal_match() {
        let c1 = Constraint::new(Operator::Equal, "1.0.0.0".to_string()).unwrap();
        let c2 = Constraint::new(Operator::Equal, "1.0.0.0".to_string()).unwrap();
        assert!(c1.match_specific(&c2, false));
    }

    // Helper function to test constraint matching
    fn test_match(req_op: Operator, req_ver: &str, prov_op: Operator, prov_ver: &str) -> bool {
        let require = Constraint::new(req_op, req_ver.to_string()).unwrap();
        let provide = Constraint::new(prov_op, prov_ver.to_string()).unwrap();
        require.match_specific(&provide, false)
    }

    #[test]
    fn test_version_match_succeeds_equal() {
        // == matches
        assert!(test_match(Operator::Equal, "2", Operator::Equal, "2"));
        assert!(test_match(Operator::Equal, "2", Operator::LessThan, "3"));
        assert!(test_match(Operator::Equal, "2", Operator::LessThanOrEqual, "2"));
        assert!(test_match(Operator::Equal, "2", Operator::LessThanOrEqual, "3"));
        assert!(test_match(Operator::Equal, "2", Operator::GreaterThanOrEqual, "1"));
        assert!(test_match(Operator::Equal, "2", Operator::GreaterThanOrEqual, "2"));
        assert!(test_match(Operator::Equal, "2", Operator::GreaterThan, "1"));
        assert!(test_match(Operator::Equal, "2", Operator::NotEqual, "1"));
        assert!(test_match(Operator::Equal, "2", Operator::NotEqual, "3"));
    }

    #[test]
    fn test_version_match_succeeds_less_than() {
        // < matches
        assert!(test_match(Operator::LessThan, "2", Operator::Equal, "1"));
        assert!(test_match(Operator::LessThan, "2", Operator::LessThan, "1"));
        assert!(test_match(Operator::LessThan, "2", Operator::LessThan, "2"));
        assert!(test_match(Operator::LessThan, "2", Operator::LessThan, "3"));
        assert!(test_match(Operator::LessThan, "2", Operator::LessThanOrEqual, "1"));
        assert!(test_match(Operator::LessThan, "2", Operator::LessThanOrEqual, "2"));
        assert!(test_match(Operator::LessThan, "2", Operator::LessThanOrEqual, "3"));
        assert!(test_match(Operator::LessThan, "2", Operator::GreaterThanOrEqual, "1"));
        assert!(test_match(Operator::LessThan, "2", Operator::GreaterThan, "1"));
        assert!(test_match(Operator::LessThan, "2", Operator::NotEqual, "1"));
        assert!(test_match(Operator::LessThan, "2", Operator::NotEqual, "2"));
        assert!(test_match(Operator::LessThan, "2", Operator::NotEqual, "3"));
    }

    #[test]
    fn test_version_match_succeeds_less_than_or_equal() {
        // <= matches
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::Equal, "1"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::Equal, "2"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::LessThan, "1"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::LessThan, "2"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::LessThan, "3"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::LessThanOrEqual, "1"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::LessThanOrEqual, "2"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::LessThanOrEqual, "3"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::GreaterThanOrEqual, "1"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::GreaterThanOrEqual, "2"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::GreaterThan, "1"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::NotEqual, "1"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::NotEqual, "2"));
        assert!(test_match(Operator::LessThanOrEqual, "2", Operator::NotEqual, "3"));
    }

    #[test]
    fn test_version_match_succeeds_greater_than_or_equal() {
        // >= matches
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::Equal, "2"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::Equal, "3"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::LessThan, "3"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::LessThanOrEqual, "2"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::LessThanOrEqual, "3"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::GreaterThanOrEqual, "1"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::GreaterThanOrEqual, "2"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::GreaterThanOrEqual, "3"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::GreaterThan, "1"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::GreaterThan, "2"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::GreaterThan, "3"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::NotEqual, "1"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::NotEqual, "2"));
        assert!(test_match(Operator::GreaterThanOrEqual, "2", Operator::NotEqual, "3"));
    }

    #[test]
    fn test_version_match_succeeds_greater_than() {
        // > matches
        assert!(test_match(Operator::GreaterThan, "2", Operator::Equal, "3"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::LessThan, "3"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::LessThanOrEqual, "3"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::GreaterThanOrEqual, "1"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::GreaterThanOrEqual, "2"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::GreaterThanOrEqual, "3"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::GreaterThan, "1"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::GreaterThan, "2"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::GreaterThan, "3"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::NotEqual, "1"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::NotEqual, "2"));
        assert!(test_match(Operator::GreaterThan, "2", Operator::NotEqual, "3"));
    }

    #[test]
    fn test_version_match_succeeds_not_equal() {
        // != matches
        assert!(test_match(Operator::NotEqual, "2", Operator::NotEqual, "1"));
        assert!(test_match(Operator::NotEqual, "2", Operator::NotEqual, "2"));
        assert!(test_match(Operator::NotEqual, "2", Operator::NotEqual, "3"));
        assert!(test_match(Operator::NotEqual, "2", Operator::Equal, "1"));
        assert!(test_match(Operator::NotEqual, "2", Operator::Equal, "3"));
        assert!(test_match(Operator::NotEqual, "2", Operator::LessThan, "1"));
        assert!(test_match(Operator::NotEqual, "2", Operator::LessThan, "2"));
        assert!(test_match(Operator::NotEqual, "2", Operator::LessThan, "3"));
        assert!(test_match(Operator::NotEqual, "2", Operator::LessThanOrEqual, "1"));
        assert!(test_match(Operator::NotEqual, "2", Operator::LessThanOrEqual, "2"));
        assert!(test_match(Operator::NotEqual, "2", Operator::LessThanOrEqual, "3"));
        assert!(test_match(Operator::NotEqual, "2", Operator::GreaterThanOrEqual, "1"));
        assert!(test_match(Operator::NotEqual, "2", Operator::GreaterThanOrEqual, "2"));
        assert!(test_match(Operator::NotEqual, "2", Operator::GreaterThanOrEqual, "3"));
        assert!(test_match(Operator::NotEqual, "2", Operator::GreaterThan, "1"));
        assert!(test_match(Operator::NotEqual, "2", Operator::GreaterThan, "2"));
        assert!(test_match(Operator::NotEqual, "2", Operator::GreaterThan, "3"));
    }

    #[test]
    fn test_version_match_succeeds_branches() {
        // Branch names
        assert!(test_match(Operator::Equal, "dev-foo-bar", Operator::Equal, "dev-foo-bar"));
        assert!(test_match(Operator::Equal, "dev-events+issue-17", Operator::Equal, "dev-events+issue-17"));
        assert!(test_match(Operator::Equal, "dev-foo-bar", Operator::NotEqual, "dev-foo-xyz"));
        assert!(test_match(Operator::NotEqual, "dev-foo-bar", Operator::NotEqual, "dev-foo-xyz"));
    }

    #[test]
    fn test_version_match_succeeds_numbers_vs_branches() {
        // Numbers vs branches
        assert!(test_match(Operator::Equal, "0.12", Operator::NotEqual, "dev-foo"));
        assert!(test_match(Operator::LessThan, "0.12", Operator::NotEqual, "dev-foo"));
        assert!(test_match(Operator::LessThanOrEqual, "0.12", Operator::NotEqual, "dev-foo"));
        assert!(test_match(Operator::GreaterThanOrEqual, "0.12", Operator::NotEqual, "dev-foo"));
        assert!(test_match(Operator::GreaterThan, "0.12", Operator::NotEqual, "dev-foo"));
        assert!(test_match(Operator::NotEqual, "0.12", Operator::Equal, "dev-foo"));
        assert!(test_match(Operator::NotEqual, "0.12", Operator::NotEqual, "dev-foo"));
    }

    #[test]
    fn test_version_match_fails_equal() {
        // == fails
        assert!(!test_match(Operator::Equal, "2", Operator::Equal, "1"));
        assert!(!test_match(Operator::Equal, "2", Operator::Equal, "3"));
        assert!(!test_match(Operator::Equal, "2", Operator::LessThan, "1"));
        assert!(!test_match(Operator::Equal, "2", Operator::LessThan, "2"));
        assert!(!test_match(Operator::Equal, "2", Operator::LessThanOrEqual, "1"));
        assert!(!test_match(Operator::Equal, "2", Operator::GreaterThanOrEqual, "3"));
        assert!(!test_match(Operator::Equal, "2", Operator::GreaterThan, "2"));
        assert!(!test_match(Operator::Equal, "2", Operator::GreaterThan, "3"));
        assert!(!test_match(Operator::Equal, "2", Operator::NotEqual, "2"));
    }

    #[test]
    fn test_version_match_fails_less_than() {
        // < fails
        assert!(!test_match(Operator::LessThan, "2", Operator::Equal, "2"));
        assert!(!test_match(Operator::LessThan, "2", Operator::Equal, "3"));
        assert!(!test_match(Operator::LessThan, "2", Operator::GreaterThanOrEqual, "2"));
        assert!(!test_match(Operator::LessThan, "2", Operator::GreaterThanOrEqual, "3"));
        assert!(!test_match(Operator::LessThan, "2", Operator::GreaterThan, "2"));
        assert!(!test_match(Operator::LessThan, "2", Operator::GreaterThan, "3"));
    }

    #[test]
    fn test_version_match_fails_less_than_or_equal() {
        // <= fails
        assert!(!test_match(Operator::LessThanOrEqual, "2", Operator::Equal, "3"));
        assert!(!test_match(Operator::LessThanOrEqual, "2", Operator::GreaterThanOrEqual, "3"));
        assert!(!test_match(Operator::LessThanOrEqual, "2", Operator::GreaterThan, "2"));
        assert!(!test_match(Operator::LessThanOrEqual, "2", Operator::GreaterThan, "3"));
    }

    #[test]
    fn test_version_match_fails_greater_than_or_equal() {
        // >= fails
        assert!(!test_match(Operator::GreaterThanOrEqual, "2", Operator::Equal, "1"));
        assert!(!test_match(Operator::GreaterThanOrEqual, "2", Operator::LessThan, "1"));
        assert!(!test_match(Operator::GreaterThanOrEqual, "2", Operator::LessThan, "2"));
        assert!(!test_match(Operator::GreaterThanOrEqual, "2", Operator::LessThanOrEqual, "1"));
    }

    #[test]
    fn test_version_match_fails_greater_than() {
        // > fails
        assert!(!test_match(Operator::GreaterThan, "2", Operator::Equal, "1"));
        assert!(!test_match(Operator::GreaterThan, "2", Operator::Equal, "2"));
        assert!(!test_match(Operator::GreaterThan, "2", Operator::LessThan, "1"));
        assert!(!test_match(Operator::GreaterThan, "2", Operator::LessThan, "2"));
        assert!(!test_match(Operator::GreaterThan, "2", Operator::LessThanOrEqual, "1"));
        assert!(!test_match(Operator::GreaterThan, "2", Operator::LessThanOrEqual, "2"));
    }

    #[test]
    fn test_version_match_fails_not_equal() {
        // != fails
        assert!(!test_match(Operator::NotEqual, "2", Operator::Equal, "2"));
    }

    #[test]
    fn test_version_match_fails_branches() {
        // Different branch names
        assert!(!test_match(Operator::Equal, "dev-foo-bar", Operator::Equal, "dev-foo-xyz"));
        assert!(!test_match(Operator::Equal, "dev-foo-bar", Operator::LessThan, "dev-foo-xyz"));
        assert!(!test_match(Operator::Equal, "dev-foo-bar", Operator::LessThanOrEqual, "dev-foo-xyz"));
        assert!(!test_match(Operator::Equal, "dev-foo-bar", Operator::GreaterThanOrEqual, "dev-foo-xyz"));
        assert!(!test_match(Operator::Equal, "dev-foo-bar", Operator::GreaterThan, "dev-foo-xyz"));

        // Same branch - non-equal operators always fail
        assert!(!test_match(Operator::Equal, "dev-foo-bar", Operator::NotEqual, "dev-foo-bar"));
        assert!(!test_match(Operator::LessThan, "dev-foo-bar", Operator::Equal, "dev-foo-bar"));
        assert!(!test_match(Operator::LessThan, "dev-foo-bar", Operator::LessThan, "dev-foo-bar"));
        assert!(!test_match(Operator::GreaterThan, "dev-foo-bar", Operator::Equal, "dev-foo-bar"));
        assert!(!test_match(Operator::GreaterThan, "dev-foo-bar", Operator::GreaterThan, "dev-foo-bar"));
    }

    #[test]
    fn test_version_match_fails_numbers_vs_branches() {
        // Branch vs number, not comparable so mostly false
        assert!(!test_match(Operator::Equal, "0.12", Operator::Equal, "dev-foo"));
        assert!(!test_match(Operator::Equal, "0.12", Operator::LessThan, "dev-foo"));
        assert!(!test_match(Operator::Equal, "0.12", Operator::LessThanOrEqual, "dev-foo"));
        assert!(!test_match(Operator::Equal, "0.12", Operator::GreaterThanOrEqual, "dev-foo"));
        assert!(!test_match(Operator::Equal, "0.12", Operator::GreaterThan, "dev-foo"));

        assert!(!test_match(Operator::LessThan, "0.12", Operator::Equal, "dev-foo"));
        assert!(!test_match(Operator::LessThan, "0.12", Operator::LessThan, "dev-foo"));
        assert!(!test_match(Operator::LessThan, "0.12", Operator::LessThanOrEqual, "dev-foo"));
        assert!(!test_match(Operator::LessThan, "0.12", Operator::GreaterThanOrEqual, "dev-foo"));
        assert!(!test_match(Operator::LessThan, "0.12", Operator::GreaterThan, "dev-foo"));

        assert!(!test_match(Operator::GreaterThan, "0.12", Operator::Equal, "dev-foo"));
        assert!(!test_match(Operator::GreaterThan, "0.12", Operator::LessThan, "dev-foo"));
        assert!(!test_match(Operator::GreaterThan, "0.12", Operator::LessThanOrEqual, "dev-foo"));
        assert!(!test_match(Operator::GreaterThan, "0.12", Operator::GreaterThanOrEqual, "dev-foo"));
        assert!(!test_match(Operator::GreaterThan, "0.12", Operator::GreaterThan, "dev-foo"));
    }

    #[test]
    fn test_bounds_comprehensive() {
        // Equal bounds
        let c = Constraint::new(Operator::Equal, "1.0.0.0".to_string()).unwrap();
        assert_eq!(c.lower_bound().version(), "1.0.0.0");
        assert!(c.lower_bound().is_inclusive());
        assert_eq!(c.upper_bound().version(), "1.0.0.0");
        assert!(c.upper_bound().is_inclusive());

        // Less than bounds
        let c = Constraint::new(Operator::LessThan, "1.0.0.0".to_string()).unwrap();
        assert!(c.lower_bound().is_zero());
        assert_eq!(c.upper_bound().version(), "1.0.0.0");
        assert!(!c.upper_bound().is_inclusive());

        // Less than or equal bounds
        let c = Constraint::new(Operator::LessThanOrEqual, "1.0.0.0".to_string()).unwrap();
        assert!(c.lower_bound().is_zero());
        assert_eq!(c.upper_bound().version(), "1.0.0.0");
        assert!(c.upper_bound().is_inclusive());

        // Greater than bounds
        let c = Constraint::new(Operator::GreaterThan, "1.0.0.0".to_string()).unwrap();
        assert_eq!(c.lower_bound().version(), "1.0.0.0");
        assert!(!c.lower_bound().is_inclusive());
        assert!(c.upper_bound().is_positive_infinity());

        // Greater than or equal bounds
        let c = Constraint::new(Operator::GreaterThanOrEqual, "1.0.0.0".to_string()).unwrap();
        assert_eq!(c.lower_bound().version(), "1.0.0.0");
        assert!(c.lower_bound().is_inclusive());
        assert!(c.upper_bound().is_positive_infinity());

        // Not equal bounds (infinite range)
        let c = Constraint::new(Operator::NotEqual, "1.0.0.0".to_string()).unwrap();
        assert!(c.lower_bound().is_zero());
        assert!(c.upper_bound().is_positive_infinity());

        // Dev branch bounds (infinite range)
        let c = Constraint::new(Operator::GreaterThanOrEqual, "dev-feature-branch".to_string()).unwrap();
        assert!(c.lower_bound().is_zero());
        assert!(c.upper_bound().is_positive_infinity());
    }

    #[test]
    fn test_dev_version_suffix_matches_constraint() {
        // This is how Composer represents branch versions like 6.7.x-dev
        // 6.7.9999999.9999999-dev should match >=6.7.2.0

        // Direct version comparison
        assert!(php_version_compare("6.7.9999999.9999999-dev", "6.7.2.0", ">="),
            "6.7.9999999.9999999-dev should be >= 6.7.2.0");

        // Through constraint matching
        let require = Constraint::new(Operator::GreaterThanOrEqual, "6.7.2.0".to_string()).unwrap();
        let provide = Constraint::new(Operator::Equal, "6.7.9999999.9999999-dev".to_string()).unwrap();
        assert!(require.match_specific(&provide, false),
            ">=6.7.2.0 should match =6.7.9999999.9999999-dev");
    }
}
