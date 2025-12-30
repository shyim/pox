//! MatchAllConstraint - matches any version

use std::fmt;

use super::{Bound, ConstraintInterface};

/// A constraint that matches any version
#[derive(Debug, Clone)]
pub struct MatchAllConstraint {
    pretty_string: Option<String>,
}

impl MatchAllConstraint {
    /// Create a new MatchAllConstraint
    pub fn new() -> Self {
        MatchAllConstraint {
            pretty_string: None,
        }
    }
}

impl Default for MatchAllConstraint {
    fn default() -> Self {
        Self::new()
    }
}

impl ConstraintInterface for MatchAllConstraint {
    fn matches(&self, _other: &dyn ConstraintInterface) -> bool {
        true
    }

    fn lower_bound(&self) -> Bound {
        Bound::zero()
    }

    fn upper_bound(&self) -> Bound {
        Bound::positive_infinity()
    }

    fn pretty_string(&self) -> String {
        self.pretty_string.clone().unwrap_or_else(|| "*".to_string())
    }

    fn set_pretty_string(&mut self, pretty: Option<String>) {
        self.pretty_string = pretty;
    }

    fn clone_box(&self) -> Box<dyn ConstraintInterface> {
        Box::new(self.clone())
    }

    fn is_match_all(&self) -> bool {
        true
    }
}

impl fmt::Display for MatchAllConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "*")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::{Constraint, Operator};

    #[test]
    fn test_match_all_matches_everything() {
        let match_all = MatchAllConstraint::new();
        let constraint = Constraint::new(Operator::Equal, "1.0.0".to_string()).unwrap();

        assert!(match_all.matches(&constraint));
    }

    #[test]
    fn test_match_all_display() {
        let match_all = MatchAllConstraint::new();
        assert_eq!(match_all.to_string(), "*");
    }

    #[test]
    fn test_match_all_bounds() {
        let match_all = MatchAllConstraint::new();
        assert!(match_all.lower_bound().is_zero());
        assert!(match_all.upper_bound().is_positive_infinity());
    }
}
