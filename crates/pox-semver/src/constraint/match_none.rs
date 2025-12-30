//! MatchNoneConstraint - matches no version

use std::fmt;

use super::{Bound, ConstraintInterface};

/// A constraint that matches no version
#[derive(Debug, Clone)]
pub struct MatchNoneConstraint {
    pretty_string: Option<String>,
}

impl MatchNoneConstraint {
    /// Create a new MatchNoneConstraint
    pub fn new() -> Self {
        MatchNoneConstraint {
            pretty_string: None,
        }
    }
}

impl Default for MatchNoneConstraint {
    fn default() -> Self {
        Self::new()
    }
}

impl ConstraintInterface for MatchNoneConstraint {
    fn matches(&self, _other: &dyn ConstraintInterface) -> bool {
        false
    }

    fn lower_bound(&self) -> Bound {
        Bound::new("0.0.0.0-dev".to_string(), false)
    }

    fn upper_bound(&self) -> Bound {
        Bound::new("0.0.0.0-dev".to_string(), false)
    }

    fn pretty_string(&self) -> String {
        self.pretty_string.clone().unwrap_or_else(|| "[]".to_string())
    }

    fn set_pretty_string(&mut self, pretty: Option<String>) {
        self.pretty_string = pretty;
    }

    fn clone_box(&self) -> Box<dyn ConstraintInterface> {
        Box::new(self.clone())
    }

    fn is_match_none(&self) -> bool {
        true
    }
}

impl fmt::Display for MatchNoneConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::{Constraint, Operator};

    #[test]
    fn test_match_none_matches_nothing() {
        let match_none = MatchNoneConstraint::new();
        let constraint = Constraint::new(Operator::Equal, "1.0.0".to_string()).unwrap();

        assert!(!match_none.matches(&constraint));
    }

    #[test]
    fn test_match_none_display() {
        let match_none = MatchNoneConstraint::new();
        assert_eq!(match_none.to_string(), "[]");
    }
}
