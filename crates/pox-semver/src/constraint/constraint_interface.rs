//! Constraint interface trait

use super::Bound;
use super::Operator;

/// Trait for all constraint types
pub trait ConstraintInterface: std::fmt::Debug + std::fmt::Display + Send + Sync {
    /// Check if this constraint matches another constraint
    fn matches(&self, other: &dyn ConstraintInterface) -> bool;

    /// Get the lower bound of this constraint
    fn lower_bound(&self) -> Bound;

    /// Get the upper bound of this constraint
    fn upper_bound(&self) -> Bound;

    /// Get the pretty string representation
    fn pretty_string(&self) -> String;

    /// Set the pretty string representation
    fn set_pretty_string(&mut self, pretty: Option<String>);

    /// Clone this constraint into a boxed trait object
    fn clone_box(&self) -> Box<dyn ConstraintInterface>;

    /// Check if this is a Constraint (single version constraint)
    fn as_constraint(&self) -> Option<(&Operator, &str)> {
        None
    }

    /// Check if this is a MatchAllConstraint
    fn is_match_all(&self) -> bool {
        false
    }

    /// Check if this is a MatchNoneConstraint
    fn is_match_none(&self) -> bool {
        false
    }

    /// Check if this is a MultiConstraint
    fn as_multi_constraint(&self) -> Option<(&[Box<dyn ConstraintInterface>], bool)> {
        None
    }
}

impl Clone for Box<dyn ConstraintInterface> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}
