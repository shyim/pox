//! Bound type for constraint boundaries

use std::cmp::Ordering;
use std::fmt;

/// Represents a bound (lower or upper) of a version constraint
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    version: String,
    is_inclusive: bool,
}

impl Bound {
    /// Create a new bound
    pub fn new(version: String, is_inclusive: bool) -> Self {
        Bound {
            version,
            is_inclusive,
        }
    }

    /// Get the version string
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Check if the bound is inclusive
    pub fn is_inclusive(&self) -> bool {
        self.is_inclusive
    }

    /// Check if this is the zero bound
    pub fn is_zero(&self) -> bool {
        self.version == "0.0.0.0-dev" && self.is_inclusive
    }

    /// Check if this is positive infinity
    pub fn is_positive_infinity(&self) -> bool {
        self.version == format!("{}.0.0.0", i64::MAX) && !self.is_inclusive
    }

    /// Create the zero bound (minimum possible version)
    pub fn zero() -> Self {
        Bound {
            version: "0.0.0.0-dev".to_string(),
            is_inclusive: true,
        }
    }

    /// Create positive infinity bound (maximum possible version)
    pub fn positive_infinity() -> Self {
        Bound {
            version: format!("{}.0.0.0", i64::MAX),
            is_inclusive: false,
        }
    }

    /// Compare this bound to another with a given operator
    pub fn compare_to(&self, other: &Bound, operator: &str) -> bool {
        if operator != "<" && operator != ">" {
            panic!("Does not support any other operator other than > or <");
        }

        // If they are equal, return false
        if self == other {
            return false;
        }

        let compare_result = version_compare(&self.version, &other.version);

        // Not the same version means we don't need to check inclusivity
        if compare_result != Ordering::Equal {
            let target = if operator == ">" {
                Ordering::Greater
            } else {
                Ordering::Less
            };
            return compare_result == target;
        }

        // Question: "am I higher than other?"
        if operator == ">" {
            other.is_inclusive
        } else {
            !other.is_inclusive
        }
    }
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}]",
            self.version,
            if self.is_inclusive {
                "inclusive"
            } else {
                "exclusive"
            }
        )
    }
}

/// Compare two version strings
fn version_compare(a: &str, b: &str) -> Ordering {
    // Split versions into comparable parts
    let a_parts = parse_version_parts(a);
    let b_parts = parse_version_parts(b);

    // Compare numeric parts first
    let max_len = std::cmp::max(a_parts.numeric.len(), b_parts.numeric.len());
    for i in 0..max_len {
        let a_val = a_parts.numeric.get(i).copied().unwrap_or(0);
        let b_val = b_parts.numeric.get(i).copied().unwrap_or(0);
        match a_val.cmp(&b_val) {
            Ordering::Equal => continue,
            other => return other,
        }
    }

    // Compare stability
    let a_stability = stability_order(&a_parts.stability);
    let b_stability = stability_order(&b_parts.stability);
    match a_stability.cmp(&b_stability) {
        Ordering::Equal => {}
        other => return other,
    }

    // Compare stability version
    a_parts.stability_version.cmp(&b_parts.stability_version)
}

#[derive(Debug, Default)]
struct VersionParts {
    numeric: Vec<i64>,
    stability: String,
    stability_version: i64,
}

fn parse_version_parts(version: &str) -> VersionParts {
    let mut parts = VersionParts::default();

    // Handle dev- prefix
    if version.starts_with("dev-") {
        parts.stability = "dev".to_string();
        return parts;
    }

    // Split on dash for stability
    let (numeric_part, stability_part) = if let Some(pos) = version.find('-') {
        (&version[..pos], Some(&version[pos + 1..]))
    } else {
        (version, None)
    };

    // Parse numeric parts
    for part in numeric_part.split('.') {
        if let Ok(n) = part.parse::<i64>() {
            parts.numeric.push(n);
        }
    }

    // Parse stability
    if let Some(stability) = stability_part {
        let stability_lower = stability.to_lowercase();
        if stability_lower.starts_with("dev") {
            parts.stability = "dev".to_string();
        } else if stability_lower.starts_with("alpha") {
            parts.stability = "alpha".to_string();
            parts.stability_version = parse_stability_version(&stability_lower, "alpha");
        } else if stability_lower.starts_with("beta") {
            parts.stability = "beta".to_string();
            parts.stability_version = parse_stability_version(&stability_lower, "beta");
        } else if stability_lower.starts_with("rc") {
            parts.stability = "rc".to_string();
            parts.stability_version = parse_stability_version(&stability_lower, "rc");
        } else if stability_lower.starts_with("patch") {
            parts.stability = "patch".to_string();
            parts.stability_version = parse_stability_version(&stability_lower, "patch");
        } else {
            parts.stability = stability_lower;
        }
    } else {
        parts.stability = "stable".to_string();
    }

    parts
}

fn parse_stability_version(stability: &str, prefix: &str) -> i64 {
    let rest = &stability[prefix.len()..];
    rest.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0)
}

fn stability_order(stability: &str) -> i32 {
    match stability {
        "dev" => 0,
        "alpha" => 1,
        "beta" => 2,
        "rc" => 3,
        "stable" | "" => 4,
        "patch" => 5,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bound_creation() {
        let bound = Bound::new("1.0.0.0".to_string(), true);
        assert_eq!(bound.version(), "1.0.0.0");
        assert!(bound.is_inclusive());
    }

    #[test]
    fn test_zero_bound() {
        let zero = Bound::zero();
        assert!(zero.is_zero());
        assert!(!zero.is_positive_infinity());
    }

    #[test]
    fn test_positive_infinity() {
        let inf = Bound::positive_infinity();
        assert!(inf.is_positive_infinity());
        assert!(!inf.is_zero());
    }

    #[test]
    fn test_compare_to() {
        let b1 = Bound::new("1.0.0.0".to_string(), true);
        let b2 = Bound::new("2.0.0.0".to_string(), true);

        assert!(b2.compare_to(&b1, ">"));
        assert!(!b1.compare_to(&b2, ">"));
        assert!(b1.compare_to(&b2, "<"));
        assert!(!b2.compare_to(&b1, "<"));
    }
}
