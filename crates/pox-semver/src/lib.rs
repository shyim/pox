//! Semantic versioning library compatible with Composer/semver
//!
//! This crate provides semantic version parsing, comparison, and constraint matching
//! compatible with PHP's Composer package manager.

pub mod constraint;
mod comparator;
mod semver;
mod version_parser;

pub use comparator::Comparator;
pub use constraint::{Bound, Constraint, ConstraintInterface, MatchAllConstraint, MatchNoneConstraint, MultiConstraint, Operator};
pub use semver::Semver;
pub use version_parser::{ParsedConstraints, Stability, VersionParser, VersionParserError};
