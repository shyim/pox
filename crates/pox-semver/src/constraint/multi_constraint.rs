//! MultiConstraint - compound constraint combining multiple constraints

use std::fmt;
use thiserror::Error;

use super::{Bound, ConstraintInterface, MatchAllConstraint};

#[derive(Error, Debug)]
pub enum MultiConstraintError {
    #[error("Must provide at least two constraints for a MultiConstraint")]
    TooFewConstraints,
}

/// A constraint combining multiple constraints with AND (conjunctive) or OR (disjunctive) logic
#[derive(Debug, Clone)]
pub struct MultiConstraint {
    constraints: Vec<Box<dyn ConstraintInterface>>,
    conjunctive: bool,
    pretty_string: Option<String>,
    cached_string: Option<String>,
    lower_bound: Option<Bound>,
    upper_bound: Option<Bound>,
}

impl MultiConstraint {
    /// Create a new MultiConstraint
    pub fn new(
        constraints: Vec<Box<dyn ConstraintInterface>>,
        conjunctive: bool,
    ) -> Result<Self, MultiConstraintError> {
        if constraints.len() < 2 {
            return Err(MultiConstraintError::TooFewConstraints);
        }

        Ok(MultiConstraint {
            constraints,
            conjunctive,
            pretty_string: None,
            cached_string: None,
            lower_bound: None,
            upper_bound: None,
        })
    }

    /// Create a MultiConstraint, optimizing where possible
    pub fn create(
        constraints: Vec<Box<dyn ConstraintInterface>>,
        conjunctive: bool,
    ) -> Result<Box<dyn ConstraintInterface>, MultiConstraintError> {
        if constraints.is_empty() {
            return Ok(Box::new(MatchAllConstraint::new()));
        }

        if constraints.len() == 1 {
            return Ok(constraints.into_iter().next().unwrap());
        }

        // Try to optimize
        if let Some((optimized, new_conjunctive)) = Self::optimize_constraints(&constraints, conjunctive) {
            if optimized.len() == 1 {
                return Ok(optimized.into_iter().next().unwrap());
            }
            return Ok(Box::new(MultiConstraint {
                constraints: optimized,
                conjunctive: new_conjunctive,
                pretty_string: None,
                cached_string: None,
                lower_bound: None,
                upper_bound: None,
            }));
        }

        Ok(Box::new(MultiConstraint {
            constraints,
            conjunctive,
            pretty_string: None,
            cached_string: None,
            lower_bound: None,
            upper_bound: None,
        }))
    }

    /// Get the constraints
    pub fn constraints(&self) -> &[Box<dyn ConstraintInterface>] {
        &self.constraints
    }

    /// Check if this is a conjunctive (AND) constraint
    pub fn is_conjunctive(&self) -> bool {
        self.conjunctive
    }

    /// Check if this is a disjunctive (OR) constraint
    pub fn is_disjunctive(&self) -> bool {
        !self.conjunctive
    }

    fn optimize_constraints(
        constraints: &[Box<dyn ConstraintInterface>],
        conjunctive: bool,
    ) -> Option<(Vec<Box<dyn ConstraintInterface>>, bool)> {
        // Optimization for disjunctive constraints
        // [>= 1 < 2] || [>= 2 < 3] || [>= 3 < 4] => [>= 1 < 4]
        if !conjunctive && constraints.len() >= 2 {
            // Check if we can merge adjacent ranges
            // This is a simplified version - full implementation would need more complex logic
            return None;
        }

        None
    }

    fn extract_bounds(&mut self) {
        if self.lower_bound.is_some() {
            return;
        }

        for (i, constraint) in self.constraints.iter().enumerate() {
            if i == 0 {
                self.lower_bound = Some(constraint.lower_bound());
                self.upper_bound = Some(constraint.upper_bound());
                continue;
            }

            let constraint_lower = constraint.lower_bound();
            let constraint_upper = constraint.upper_bound();

            if let Some(ref current_lower) = self.lower_bound {
                let cmp_op = if self.conjunctive { ">" } else { "<" };
                if constraint_lower.compare_to(current_lower, cmp_op) {
                    self.lower_bound = Some(constraint_lower);
                }
            }

            if let Some(ref current_upper) = self.upper_bound {
                let cmp_op = if self.conjunctive { "<" } else { ">" };
                if constraint_upper.compare_to(current_upper, cmp_op) {
                    self.upper_bound = Some(constraint_upper);
                }
            }
        }
    }
}

impl ConstraintInterface for MultiConstraint {
    fn matches(&self, provider: &dyn ConstraintInterface) -> bool {
        if self.conjunctive {
            // For disjunctive multi constraints, we need special handling
            if let Some((constraints, is_conjunctive)) = provider.as_multi_constraint() {
                if !is_conjunctive {
                    // When matching conjunctive against disjunctive, iterate over disjunctive
                    return provider.matches(self);
                }
                // Otherwise, check each constraint
                for constraint in constraints {
                    if !provider.matches(constraint.as_ref()) {
                        return false;
                    }
                }
                return true;
            }

            // AND logic - all constraints must match
            for constraint in &self.constraints {
                if !provider.matches(constraint.as_ref()) {
                    return false;
                }
            }
            true
        } else {
            // OR logic - at least one constraint must match
            for constraint in &self.constraints {
                if provider.matches(constraint.as_ref()) {
                    return true;
                }
            }
            false
        }
    }

    fn lower_bound(&self) -> Bound {
        let mut s = self.clone();
        s.extract_bounds();
        s.lower_bound.unwrap_or_else(Bound::zero)
    }

    fn upper_bound(&self) -> Bound {
        let mut s = self.clone();
        s.extract_bounds();
        s.upper_bound.unwrap_or_else(Bound::positive_infinity)
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

    fn as_multi_constraint(&self) -> Option<(&[Box<dyn ConstraintInterface>], bool)> {
        Some((&self.constraints, self.conjunctive))
    }
}

impl fmt::Display for MultiConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref cached) = self.cached_string {
            return write!(f, "{}", cached);
        }

        let constraints_str: Vec<String> = self.constraints.iter().map(|c| c.to_string()).collect();

        let separator = if self.conjunctive { " " } else { " || " };
        write!(f, "[{}]", constraints_str.join(separator))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::{Constraint, Operator};

    #[test]
    fn test_multi_constraint_conjunctive() {
        let c1 = Box::new(Constraint::new(Operator::GreaterThanOrEqual, "1.0.0".to_string()).unwrap());
        let c2 = Box::new(Constraint::new(Operator::LessThan, "2.0.0".to_string()).unwrap());

        let multi = MultiConstraint::new(vec![c1, c2], true).unwrap();
        assert!(multi.is_conjunctive());
        assert!(!multi.is_disjunctive());
    }

    #[test]
    fn test_multi_constraint_disjunctive() {
        let c1 = Box::new(Constraint::new(Operator::Equal, "1.0.0".to_string()).unwrap());
        let c2 = Box::new(Constraint::new(Operator::Equal, "2.0.0".to_string()).unwrap());

        let multi = MultiConstraint::new(vec![c1, c2], false).unwrap();
        assert!(!multi.is_conjunctive());
        assert!(multi.is_disjunctive());
    }

    #[test]
    fn test_multi_constraint_too_few() {
        let c1 = Box::new(Constraint::new(Operator::Equal, "1.0.0".to_string()).unwrap());
        let result = MultiConstraint::new(vec![c1], true);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_single_constraint() {
        let c1: Box<dyn ConstraintInterface> = Box::new(Constraint::new(Operator::Equal, "1.0.0".to_string()).unwrap());
        let result = MultiConstraint::create(vec![c1], true).unwrap();
        // Should return the single constraint, not wrapped in MultiConstraint
        assert!(result.as_constraint().is_some());
    }

    #[test]
    fn test_create_empty_returns_match_all() {
        let result = MultiConstraint::create(vec![], true).unwrap();
        assert!(result.is_match_all());
    }
}
