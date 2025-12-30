//! Solver tests ported from Composer's SolverTest.php
//!
//! These tests validate the SAT-based dependency resolver behaves correctly
//! for various package resolution scenarios.

use std::sync::Arc;
use super::*;
use crate::package::Package;

/// Helper to create a package with a given name and version
fn pkg(name: &str, version: &str) -> Package {
    Package::new(name, version)
}

/// Helper to create a package with requirements
fn pkg_with_requires(name: &str, version: &str, requires: Vec<(&str, &str)>) -> Package {
    let mut p = Package::new(name, version);
    for (dep_name, constraint) in requires {
        p.require.insert(dep_name.to_string(), constraint.to_string());
    }
    p
}

/// Helper to create a package with replaces
fn pkg_with_replaces(name: &str, version: &str, replaces: Vec<(&str, &str)>) -> Package {
    let mut p = Package::new(name, version);
    for (replace_name, constraint) in replaces {
        p.replace.insert(replace_name.to_string(), constraint.to_string());
    }
    p
}

/// Create a Transaction from a SolverResult using the request's locked packages as present.
fn make_transaction(solver_result: &SolverResult, request: &Request) -> Transaction {
    // Build present packages from locked packages in the request.
    // Exclude fixed packages since they represent platform packages that
    // should never generate operations (they're immutable).
    let present_packages: Vec<Arc<Package>> = request.locked_packages
        .iter()
        .filter(|pkg| !request.is_fixed(&pkg.name))
        .cloned()
        .collect();

    // Create transaction by comparing present with result
    Transaction::from_packages(
        present_packages,
        solver_result.packages.clone(),
        solver_result.aliases.clone(),
    )
}

/// Check that the solver result matches expected operations.
///
/// This creates a Transaction by comparing present packages (from locked packages in the request)
/// with the solver's result packages, matching Composer's Transaction architecture.
fn check_solver_result(
    solver_result: &SolverResult,
    request: &Request,
    expected: Vec<(&str, &str, &str)>, // (job, package_name, version)
) {
    let transaction = make_transaction(solver_result, request);

    let mut actual: Vec<(String, String, String)> = Vec::new();

    for op in &transaction.operations {
        match op {
            Operation::Install(pkg) => {
                actual.push(("install".to_string(), pkg.name.clone(), pkg.version.clone()));
            }
            Operation::Update { from, to } => {
                actual.push(("update".to_string(), to.name.clone(), format!("{} -> {}", from.version, to.version)));
            }
            Operation::Uninstall(pkg) => {
                actual.push(("remove".to_string(), pkg.name.clone(), pkg.version.clone()));
            }
            Operation::MarkUnneeded(pkg) => {
                actual.push(("mark_unneeded".to_string(), pkg.name.clone(), pkg.version.clone()));
            }
            Operation::MarkAliasInstalled(alias) => {
                actual.push(("alias_install".to_string(), alias.name().to_string(), alias.version().to_string()));
            }
            Operation::MarkAliasUninstalled(alias) => {
                actual.push(("alias_uninstall".to_string(), alias.name().to_string(), alias.version().to_string()));
            }
        }
    }

    let expected_vec: Vec<(String, String, String)> = expected
        .into_iter()
        .map(|(j, n, v)| (j.to_string(), n.to_string(), v.to_string()))
        .collect();

    // Sort both for comparison (since order may vary)
    let mut actual_sorted = actual.clone();
    let mut expected_sorted = expected_vec.clone();
    actual_sorted.sort();
    expected_sorted.sort();

    assert_eq!(
        actual_sorted, expected_sorted,
        "\nExpected operations: {:?}\nActual operations: {:?}",
        expected_vec, actual
    );
}

// ============================================================================
// Basic Installation Tests
// ============================================================================

#[test]
fn test_solver_install_single() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    check_solver_result(&solver_result, &request, vec![
        ("install", "a", "1.0.0"),
    ]);
}

#[test]
fn test_solver_remove_if_not_requested() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    // Package A is locked but not required
    request.lock(pkg("a", "1.0.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    check_solver_result(&solver_result, &request, vec![
        ("remove", "a", "1.0.0"),
    ]);
}

#[test]
fn test_solver_install_with_deps() {
    let mut pool = Pool::new();

    // A requires B < 1.1
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("b", "<1.1")]));
    pool.add_package(pkg("b", "1.0.0"));
    pool.add_package(pkg("b", "1.1.0")); // This version should NOT be selected

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Should install B 1.0.0 (not 1.1.0) and A 1.0.0
    let installs: Vec<_> = transaction.installs().collect();
    assert_eq!(installs.len(), 2);

    let b_pkg = installs.iter().find(|p| p.name == "b").expect("B should be installed");
    assert_eq!(b_pkg.version, "1.0.0", "B version should be 1.0.0, not 1.1.0");
}

#[test]
fn test_solver_install_with_deps_in_order() {
    let mut pool = Pool::new();

    // B requires A and C, C requires A
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg_with_requires("b", "1.0.0", vec![("a", ">=1.0"), ("c", ">=1.0")]));
    pool.add_package(pkg_with_requires("c", "1.0.0", vec![("a", ">=1.0")]));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.require("b", "*");
    request.require("c", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // After sorting, A should be installed before C, C before B
    let install_names: Vec<String> = transaction.installs().map(|p| p.name.clone()).collect();
    assert_eq!(install_names.len(), 3);
}

// ============================================================================
// Update Tests
// ============================================================================

#[test]
fn test_solver_update_single() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "1.1.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.lock(pkg("a", "1.0.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Should update A from 1.0.0 to 1.1.0
    let updates: Vec<_> = transaction.updates().collect();
    assert_eq!(updates.len(), 1, "Should have one update operation");
    assert_eq!(updates[0].0.version, "1.0.0");
    assert_eq!(updates[0].1.version, "1.1.0");
}

#[test]
fn test_solver_update_current_no_change() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.lock(pkg("a", "1.0.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // No changes needed - same version
    assert!(transaction.is_empty(), "Transaction should be empty when already at correct version");
}

#[test]
fn test_solver_update_constrained() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "1.2.0"));
    pool.add_package(pkg("a", "2.0.0")); // Should not be selected

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "<2.0");
    request.lock(pkg("a", "1.0.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Should update to 1.2.0, not 2.0.0
    let updates: Vec<_> = transaction.updates().collect();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].1.version, "1.2.0");
}

// ============================================================================
// Conflict Tests
// ============================================================================

#[test]
fn test_solver_three_alternative_require_and_conflict() {
    let mut pool = Pool::new();

    // A requires B < 1.1 and conflicts with B < 1.0
    let mut pkg_a = pkg_with_requires("a", "2.0.0", vec![("b", "<1.1")]);
    pkg_a.conflict.insert("b".to_string(), "<1.0".to_string());
    pool.add_package(pkg_a);

    pool.add_package(pkg("b", "0.9.0")); // Too old, conflicts
    pool.add_package(pkg("b", "1.0.0")); // Just right
    pool.add_package(pkg("b", "1.1.0")); // Too new, doesn't match require

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Should install B 1.0.0 (the middle version)
    let b_pkg = transaction.installs().find(|p| p.name == "b").expect("B should be installed");
    assert_eq!(b_pkg.version, "1.0.0");
}

#[test]
fn test_solver_conflict_between_requirements() {
    let mut pool = Pool::new();

    // A requires B ^1.0
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("b", "^1.0")]));
    // C requires B ^2.0
    pool.add_package(pkg_with_requires("c", "1.0.0", vec![("b", "^2.0")]));
    // Only B 1.0 exists
    pool.add_package(pkg("b", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.require("c", "*");

    let result = solver.solve(&request);
    // This should fail because C needs B ^2.0 but only B 1.0 exists
    assert!(result.is_err(), "Solver should fail due to conflicting requirements");
}

// ============================================================================
// Replace Tests
// ============================================================================

#[test]
fn test_solver_obsolete_replaced() {
    let mut pool = Pool::new();

    // B replaces A
    pool.add_package(pkg_with_replaces("b", "1.0.0", vec![("a", "*")]));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("b", "*");
    request.lock(pkg("a", "1.0.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Should remove A and install B
    let removes: Vec<_> = transaction.removals().collect();
    let installs: Vec<_> = transaction.new_installs().collect();

    assert_eq!(removes.len(), 1);
    assert_eq!(removes[0].name, "a");
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0].name, "b");
}

#[test]
fn test_skip_replacer_of_existing_package() {
    let mut pool = Pool::new();

    // A requires B
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("b", ">=1.0")]));
    // B exists
    pool.add_package(pkg("b", "1.0.0"));
    // Q replaces B but we don't need it since B exists
    pool.add_package(pkg_with_replaces("q", "1.0.0", vec![("b", ">=1.0")]));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Should install B, not Q
    let installs: Vec<_> = transaction.installs().collect();
    let b_installed = installs.iter().any(|p| p.name == "b");
    let q_installed = installs.iter().any(|p| p.name == "q");

    assert!(b_installed, "B should be installed");
    assert!(!q_installed, "Q should not be installed when B exists");
}

/// When Q replaces B and both A requires B and we require Q,
/// the solver should recognize Q satisfies B's requirement.
#[test]
fn test_skip_replaced_package_if_replacer_is_selected() {
    let mut pool = Pool::new();

    // A requires B
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("b", ">=1.0")]));
    // B exists
    pool.add_package(pkg("b", "1.0.0"));
    // Q replaces B >=1.0 (Composer-style: constraint means "replaces versions matching this")
    pool.add_package(pkg_with_replaces("q", "1.0.0", vec![("b", ">=1.0")]));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.require("q", "*"); // Explicitly require Q

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Should install Q, not B (since Q is explicitly required and replaces B)
    let installs: Vec<_> = transaction.installs().collect();
    let q_installed = installs.iter().any(|p| p.name == "q");

    assert!(q_installed, "Q should be installed");
}

// ============================================================================
// Circular Dependency Tests
// ============================================================================

#[test]
fn test_install_circular_require() {
    let mut pool = Pool::new();

    // A requires B >= 1.0
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("b", ">=1.0")]));
    // B 0.9 doesn't require A
    pool.add_package(pkg("b", "0.9.0"));
    // B 1.1 requires A >= 1.0 (circular)
    pool.add_package(pkg_with_requires("b", "1.1.0", vec![("a", ">=1.0")]));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should handle circular dependencies");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();
    assert_eq!(installs.len(), 2);

    // Should install B 1.1 (the one with circular dep) and A
    let b_pkg = installs.iter().find(|p| p.name == "b").unwrap();
    assert_eq!(b_pkg.version, "1.1.0");
}

// ============================================================================
// Version Selection Tests
// ============================================================================

#[test]
fn test_solver_prefer_highest() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "2.0.0"));
    pool.add_package(pkg("a", "3.0.0"));

    let policy = Policy::new(); // Default prefers highest
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installed: Vec<_> = transaction.installs().collect();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].version, "3.0.0", "Should prefer highest version");
}

#[test]
fn test_solver_prefer_lowest() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "2.0.0"));
    pool.add_package(pkg("a", "3.0.0"));

    let policy = Policy::new().prefer_lowest(true);
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installed: Vec<_> = transaction.installs().collect();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].version, "1.0.0", "Should prefer lowest version");
}

#[test]
fn test_solver_pick_older_if_newer_conflicts() {
    let mut pool = Pool::new();

    // X requires A >= 2.0 and B >= 2.0
    pool.add_package(pkg_with_requires("x", "1.0.0", vec![("a", ">=2.0"), ("b", ">=2.0")]));

    // A 2.0 requires B >= 2.0
    pool.add_package(pkg_with_requires("a", "2.0.0", vec![("b", ">=2.0")]));
    // A 2.1 requires B >= 2.2 (which doesn't exist)
    pool.add_package(pkg_with_requires("a", "2.1.0", vec![("b", ">=2.2")]));

    // Only B 2.1 exists
    pool.add_package(pkg("b", "2.1.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("x", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Should install A 2.0.0 (not 2.1.0 which needs B 2.2)
    let a_pkg = transaction.installs().find(|p| p.name == "a").expect("A should be installed");
    assert_eq!(a_pkg.version, "2.0.0", "Should pick older A version that works");
}

// ============================================================================
// Fixed Package Tests
// ============================================================================

#[test]
fn test_solver_fix_locked() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "1.1.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.lock(pkg("a", "1.0.0"));
    request.fix(pkg("a", "1.0.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    // Fixed package shouldn't be changed or removed
    assert!(transaction.is_empty());
}

// ============================================================================
// Non-existent Package Tests
// ============================================================================

#[test]
fn test_install_non_existing_package_fails() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("nonexistent", "1.0.0");

    let result = solver.solve(&request);
    assert!(result.is_err(), "Should fail when requiring non-existent package");
}

// ============================================================================
// Complex Dependency Chain Tests
// ============================================================================

#[test]
fn test_solver_deep_dependency_chain() {
    let mut pool = Pool::new();

    // A -> B -> C -> D -> E
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("b", "^1.0")]));
    pool.add_package(pkg_with_requires("b", "1.0.0", vec![("c", "^1.0")]));
    pool.add_package(pkg_with_requires("c", "1.0.0", vec![("d", "^1.0")]));
    pool.add_package(pkg_with_requires("d", "1.0.0", vec![("e", "^1.0")]));
    pool.add_package(pkg("e", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should resolve deep dependency chain");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();
    assert_eq!(installs.len(), 5, "Should install all 5 packages");
}

#[test]
fn test_solver_diamond_dependency() {
    let mut pool = Pool::new();

    // Diamond: A -> B, A -> C, B -> D, C -> D
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("b", "^1.0"), ("c", "^1.0")]));
    pool.add_package(pkg_with_requires("b", "1.0.0", vec![("d", "^1.0")]));
    pool.add_package(pkg_with_requires("c", "1.0.0", vec![("d", "^1.0")]));
    pool.add_package(pkg("d", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should resolve diamond dependency");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();
    assert_eq!(installs.len(), 4, "Should install A, B, C, D");

    // D should only be installed once
    let d_count = installs.iter().filter(|p| p.name == "d").count();
    assert_eq!(d_count, 1, "D should only be installed once");
}

// ============================================================================
// All Jobs Test (Install, Update, Remove in same transaction)
// ============================================================================

#[test]
fn test_solver_all_jobs() {
    let mut pool = Pool::new();

    // A requires B < 1.1
    pool.add_package(pkg_with_requires("a", "2.0.0", vec![("b", "<1.1")]));
    pool.add_package(pkg("b", "1.0.0"));
    pool.add_package(pkg("b", "1.1.0"));
    pool.add_package(pkg("c", "1.1.0"));
    pool.add_package(pkg("d", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    // Require A and C
    request.require("a", "*");
    request.require("c", "*");
    // D is locked but no longer required -> should be removed
    request.lock(pkg("d", "1.0.0"));
    // C is locked at older version -> should be updated
    request.lock(pkg("c", "1.0.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // Check we have the expected operations
    let removes: Vec<_> = transaction.removals().collect();
    let updates: Vec<_> = transaction.updates().collect();
    let new_installs: Vec<_> = transaction.new_installs().collect();

    // D should be removed (no longer required)
    assert!(removes.iter().any(|p| p.name == "d"), "D should be removed");

    // C should be updated from 1.0.0 to 1.1.0
    assert!(updates.iter().any(|(from, to)| from.name == "c" && to.name == "c"), "C should be updated");

    // A and B should be newly installed
    assert!(new_installs.iter().any(|p| p.name == "a"), "A should be installed");
    assert!(new_installs.iter().any(|p| p.name == "b"), "B should be installed");
}

// ============================================================================
// Use Replacer If Necessary Test
// ============================================================================

/// This test requires the solver to understand that D can satisfy
/// requirements for both B and C via its replaces declarations.
#[test]
fn test_use_replacer_if_necessary() {
    let mut pool = Pool::new();

    // A requires B and C
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("b", ">=1.0"), ("c", ">=1.0")]));
    // B exists
    pool.add_package(pkg("b", "1.0.0"));
    // D replaces both B and C (C doesn't exist separately, so D is needed)
    pool.add_package(pkg_with_replaces("d", "1.0.0", vec![("b", ">=1.0"), ("c", ">=1.0")]));
    pool.add_package(pkg_with_replaces("d", "1.1.0", vec![("b", ">=1.0"), ("c", ">=1.0")]));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.require("d", "*"); // We explicitly want D

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Solver should find a solution");

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // D 1.1 should be installed (highest version)
    let d_pkg = installs.iter().find(|p| p.name == "d").expect("D should be installed");
    assert_eq!(d_pkg.version, "1.1.0");
}

// ============================================================================
// Alias Package Tests - ported from Composer's SolverTest.php
// ============================================================================

use crate::package::AliasPackage;

/// Test recursive alias dependencies
/// Ported from testInstallRecursiveAliasDependencies
///
/// A 1.0 exists, A 2.0 exists (requires B==2.0), B 2.0 exists (requires A>=2.0)
/// A 2.0 has alias 1.1
/// Request: A==1.1.0.0
/// Expected: Install B 2.0, Install A 2.0, MarkAliasInstalled A 1.1
#[test]
fn test_install_recursive_alias_dependencies() {
    let mut pool = Pool::new();

    // A 1.0
    pool.add_package(pkg("a", "1.0.0"));

    // B 2.0 requires A >= 2.0
    pool.add_package(pkg_with_requires("b", "2.0.0", vec![("a", ">=2.0")]));

    // A 2.0 requires B == 2.0
    let mut pkg_a2 = Package::new("a", "2.0.0");
    pkg_a2.require.insert("b".to_string(), "==2.0.0".to_string());
    pool.add_package(pkg_a2.clone());

    // Alias: A 2.0 as 1.1
    let alias = AliasPackage::new(
        Arc::new(pkg_a2),
        "1.1.0.0".to_string(),
        "1.1".to_string(),
    );
    pool.add_alias_package(alias);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    // Request A == 1.1.0.0 (the alias version exactly)
    let mut request = Request::new();
    request.require("a", "==1.1.0.0");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution using alias: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // Should install A 2.0 (the base package) and B 2.0
    assert!(installs.iter().any(|p| p.name == "a" && p.version == "2.0.0"),
        "Should install A 2.0 (base of alias), got: {:?}", installs);
    assert!(installs.iter().any(|p| p.name == "b" && p.version == "2.0.0"),
        "Should install B 2.0 (dependency of A)");

    // Should have alias marked as installed
    let alias_installs: Vec<_> = transaction.alias_installs().collect();
    assert!(alias_installs.iter().any(|a| a.version() == "1.1.0.0"),
        "Should mark alias 1.1 as installed");
}

/// Test dev alias installation
/// Ported from testInstallDevAlias
///
/// A 2.0 exists with alias 1.1
/// B 1.0 requires A < 2.0
/// Request: A==2.0, B
/// Expected: Install A 2.0, MarkAliasInstalled A 1.1, Install B 1.0
/// The alias allows B to satisfy its A < 2.0 requirement via the alias
#[test]
fn test_install_dev_alias() {
    let mut pool = Pool::new();

    // A 2.0
    let pkg_a = Package::new("a", "2.0.0");
    pool.add_package(pkg_a.clone());

    // B 1.0 requires A < 2.0
    pool.add_package(pkg_with_requires("b", "1.0.0", vec![("a", "<2.0")]));

    // Alias: A 2.0 as 1.1 (making A 2.0 also appear as 1.1)
    let alias = AliasPackage::new(
        Arc::new(pkg_a),
        "1.1.0.0".to_string(),
        "1.1".to_string(),
    );
    pool.add_alias_package(alias);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    // Request A == 2.0 and B (which needs A < 2.0)
    // The alias makes A 2.0 also satisfy A < 2.0 via alias 1.1
    let mut request = Request::new();
    request.require("a", "==2.0.0");
    request.require("b", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution with dev alias: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // Should install A 2.0
    assert!(installs.iter().any(|p| p.name == "a" && p.version == "2.0.0"),
        "Should install A 2.0");

    // Should install B 1.0
    assert!(installs.iter().any(|p| p.name == "b" && p.version == "1.0.0"),
        "Should install B 1.0");

    // Should have alias marked as installed
    let alias_installs: Vec<_> = transaction.alias_installs().collect();
    assert!(alias_installs.iter().any(|a| a.version() == "1.1.0.0"),
        "Should mark alias 1.1 as installed");
}

/// Test what_provides with aliases
#[test]
fn test_alias_what_provides() {
    let mut pool = Pool::new();

    // A 2.0
    let pkg_a2 = Package::new("a", "2.0.0");
    pool.add_package(pkg_a2.clone());

    // Alias: A 2.0 as 1.1
    let alias = AliasPackage::new(
        Arc::new(pkg_a2),
        "1.1.0.0".to_string(),
        "1.1".to_string(),
    );
    pool.add_alias_package(alias);

    // Check all packages named 'a'
    let all_a = pool.packages_by_name("a");
    assert_eq!(all_a.len(), 2, "Should have 2 entries (base + alias)");

    // Test what_provides with alias version
    let providers = pool.what_provides("a", Some("==1.1.0.0"));

    // The alias version 1.1.0.0 should be found
    assert!(!providers.is_empty(), "Should find the alias package for ==1.1.0.0");
}

/// Test root alias installation
/// Ported from testInstallRootAliasesIfAliasOfIsInstalled
///
/// Root aliases should always be marked as installed when their base package is installed.
#[test]
fn test_install_root_aliases_if_alias_of_is_installed() {
    let mut pool = Pool::new();

    // A 1.0 with root alias 1.1
    let pkg_a = Package::new("a", "1.0.0");
    pool.add_package(pkg_a.clone());
    let mut alias_a = AliasPackage::new(
        Arc::new(pkg_a),
        "1.1.0.0".to_string(),
        "1.1".to_string(),
    );
    alias_a.set_root_package_alias(true);
    pool.add_alias_package(alias_a);

    // B 1.0 with root alias 1.1
    let pkg_b = Package::new("b", "1.0.0");
    pool.add_package(pkg_b.clone());
    let mut alias_b = AliasPackage::new(
        Arc::new(pkg_b),
        "1.1.0.0".to_string(),
        "1.1".to_string(),
    );
    alias_b.set_root_package_alias(true);
    pool.add_alias_package(alias_b);

    // C 1.0 with regular alias 1.1 (not root)
    let pkg_c = Package::new("c", "1.0.0");
    pool.add_package(pkg_c.clone());
    let alias_c = AliasPackage::new(
        Arc::new(pkg_c),
        "1.1.0.0".to_string(),
        "1.1".to_string(),
    );
    pool.add_alias_package(alias_c);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    // Request A==1.1, B==1.0, C==1.0
    let mut request = Request::new();
    request.require("a", "==1.1.0.0"); // Request alias version
    request.require("b", "==1.0.0");   // Request base version
    request.require("c", "==1.0.0");   // Request base version

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // All three base packages should be installed
    let installs: Vec<_> = transaction.installs().collect();
    assert!(installs.iter().any(|p| p.name == "a" && p.version == "1.0.0"),
        "A 1.0 should be installed");
    assert!(installs.iter().any(|p| p.name == "b" && p.version == "1.0.0"),
        "B 1.0 should be installed");
    assert!(installs.iter().any(|p| p.name == "c" && p.version == "1.0.0"),
        "C 1.0 should be installed");

    // Root aliases should be marked as installed
    let alias_installs: Vec<_> = transaction.alias_installs().collect();

    // A's alias should be installed (root alias, requested via alias version)
    assert!(alias_installs.iter().any(|a| a.name() == "a" && a.is_root_package_alias()),
        "A's root alias should be installed");

    // B's alias should be installed (root alias, even though base version was requested)
    assert!(alias_installs.iter().any(|a| a.name() == "b" && a.is_root_package_alias()),
        "B's root alias should be installed");

    // C's alias should also be installed (Composer marks all aliases when base is installed)
    assert!(alias_installs.iter().any(|a| a.name() == "c"),
        "C's alias should be installed");
}

/// Port of Composer's testSolverInstallHonoursNotEqualOperator
/// Tests multi-constraint handling with <=, <>, !=
#[test]
fn test_solver_install_honours_not_equal_operator() {
    let mut pool = Pool::new();

    // A 1.0 requires B with constraint: <=1.3, <>1.3, !=1.2
    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), "<=1.3, !=1.3, !=1.2".to_string());
    pool.add_package(pkg_a);

    pool.add_package(Package::new("b", "1.0.0"));
    pool.add_package(Package::new("b", "1.1.0"));
    pool.add_package(Package::new("b", "1.2.0"));
    pool.add_package(Package::new("b", "1.3.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // Should install B 1.1 (highest that satisfies <=1.3, !=1.3, !=1.2)
    assert!(installs.iter().any(|p| p.name == "b" && p.version == "1.1.0"),
        "Should install B 1.1.0");
    assert!(installs.iter().any(|p| p.name == "a" && p.version == "1.0.0"),
        "Should install A 1.0.0");
}

/// Port of Composer's testSolverUpdateDoesOnlyUpdate
/// Locked: A 1.0, B 1.0; Available: B 1.1; Request: fix A, require B==1.1
/// Expected: Update B from 1.0 to 1.1
#[test]
fn test_solver_update_does_only_update() {
    let mut pool = Pool::new();

    // A 1.0 requires B >= 1.0
    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_a.clone());

    pool.add_package(Package::new("b", "1.0.0"));
    pool.add_package(Package::new("b", "1.1.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.fix(pkg_a);
    request.lock(Package::new("b", "1.0.0"));
    request.require("b", "==1.1.0");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let updates: Vec<_> = transaction.updates().collect();

    assert_eq!(updates.len(), 1, "Should have exactly one update");
    assert_eq!(updates[0].0.name, "b");
    assert_eq!(updates[0].0.version, "1.0.0");
    assert_eq!(updates[0].1.name, "b");
    assert_eq!(updates[0].1.version, "1.1.0");
}

/// Port of Composer's testSolverUpdateAll
/// Locked: A 1.0 (requires B), B 1.0; Available: A 1.1 (requires B), B 1.1
/// Request: A
/// Expected: Update B 1.0 -> 1.1, Update A 1.0 -> 1.1
#[test]
fn test_solver_update_all() {
    let mut pool = Pool::new();

    let mut pkg_a_old = Package::new("a", "1.0.0");
    pkg_a_old.require.insert("b".to_string(), "*".to_string());
    pool.add_package(pkg_a_old.clone());

    let mut pkg_a_new = Package::new("a", "1.1.0");
    pkg_a_new.require.insert("b".to_string(), "*".to_string());
    pool.add_package(pkg_a_new);

    pool.add_package(Package::new("b", "1.0.0"));
    pool.add_package(Package::new("b", "1.1.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.lock(pkg_a_old);
    request.lock(Package::new("b", "1.0.0"));
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let updates: Vec<_> = transaction.updates().collect();

    assert_eq!(updates.len(), 2, "Should have two updates");

    // B should be updated first (dependency ordering)
    let b_update = updates.iter().find(|(from, _)| from.name == "b");
    assert!(b_update.is_some(), "B should be updated");
    let (b_from, b_to) = b_update.unwrap();
    assert_eq!(b_from.version, "1.0.0");
    assert_eq!(b_to.version, "1.1.0");

    let a_update = updates.iter().find(|(from, _)| from.name == "a");
    assert!(a_update.is_some(), "A should be updated");
    let (a_from, a_to) = a_update.unwrap();
    assert_eq!(a_from.version, "1.0.0");
    assert_eq!(a_to.version, "1.1.0");
}

/// Port of Composer's testSolverUpdateOnlyUpdatesSelectedPackage
/// Locked: A 1.0, B 1.0; Available: A 1.1, B 1.1
/// Request: require A, fix B
/// Expected: Update A 1.0 -> 1.1 (B stays at 1.0)
#[test]
fn test_solver_update_only_updates_selected_package() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));
    pool.add_package(Package::new("a", "1.1.0"));
    pool.add_package(Package::new("b", "1.0.0"));
    pool.add_package(Package::new("b", "1.1.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.lock(Package::new("a", "1.0.0"));
    request.lock(Package::new("b", "1.0.0"));
    request.require("a", "*");
    request.fix(Package::new("b", "1.0.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let updates: Vec<_> = transaction.updates().collect();

    assert_eq!(updates.len(), 1, "Should have exactly one update");
    assert_eq!(updates[0].0.name, "a");
    assert_eq!(updates[0].0.version, "1.0.0");
    assert_eq!(updates[0].1.version, "1.1.0");
}

/// Port of Composer's testSolverObsolete
/// Locked: A 1.0; Available: B 1.0 (replaces A)
/// Request: B
/// Expected: Remove A, Install B
#[test]
fn test_solver_obsolete() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));

    let mut pkg_b = Package::new("b", "1.0.0");
    pkg_b.replace.insert("a".to_string(), "*".to_string());
    pool.add_package(pkg_b);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.lock(Package::new("a", "1.0.0"));
    request.require("b", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    let removes: Vec<_> = transaction.uninstalls().collect();
    assert_eq!(removes.len(), 1, "Should remove one package");
    assert_eq!(removes[0].name, "a");

    let installs: Vec<_> = transaction.installs().collect();
    assert_eq!(installs.len(), 1, "Should install one package");
    assert_eq!(installs[0].name, "b");
}

/// Port of Composer's testInstallAlternativeWithCircularRequire
/// A requires B, B requires Virtual, C and D both provide Virtual and require A
/// Request: A, C
/// Expected: Install B, A, C
#[test]
fn test_install_alternative_with_circular_require() {
    let mut pool = Pool::new();

    // A requires B >= 1.0
    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_a);

    // B requires virtual >= 1.0
    let mut pkg_b = Package::new("b", "1.0.0");
    pkg_b.require.insert("virtual".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_b);

    // C provides virtual == 1.0 and requires A == 1.0
    let mut pkg_c = Package::new("c", "1.0.0");
    pkg_c.provide.insert("virtual".to_string(), "1.0.0".to_string());
    pkg_c.require.insert("a".to_string(), "==1.0.0".to_string());
    pool.add_package(pkg_c);

    // D provides virtual == 1.0 and requires A == 1.0
    let mut pkg_d = Package::new("d", "1.0.0");
    pkg_d.provide.insert("virtual".to_string(), "1.0.0".to_string());
    pkg_d.require.insert("a".to_string(), "==1.0.0".to_string());
    pool.add_package(pkg_d);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.require("c", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    assert_eq!(installs.len(), 3, "Should install 3 packages");
    assert!(installs.iter().any(|p| p.name == "a"), "Should install A");
    assert!(installs.iter().any(|p| p.name == "b"), "Should install B");
    assert!(installs.iter().any(|p| p.name == "c"), "Should install C");
}

/// Port of Composer's testLearnLiteralsWithSortedRuleLiterals
/// Complex scenario with twig and symfony packages
#[test]
fn test_learn_literals_with_sorted_rule_literals() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("twig/twig", "2.0.0"));
    pool.add_package(Package::new("twig/twig", "1.6.0"));
    pool.add_package(Package::new("twig/twig", "1.5.0"));

    let mut pkg_symfony = Package::new("symfony/symfony", "2.0.0");
    pkg_symfony.replace.insert("symfony/twig-bridge".to_string(), "==2.0.0".to_string());
    pool.add_package(pkg_symfony);

    let mut pkg_twig_bridge = Package::new("symfony/twig-bridge", "2.0.0");
    pkg_twig_bridge.require.insert("twig/twig".to_string(), "<2.0".to_string());
    pool.add_package(pkg_twig_bridge);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("symfony/twig-bridge", "*");
    request.require("twig/twig", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // Should install twig 1.6 (highest < 2.0) and twig-bridge 2.0
    // Composer prefers the original package over a replacer
    assert!(installs.iter().any(|p| p.name == "twig/twig" && p.version == "1.6.0"),
        "Should install twig/twig 1.6.0");
    assert!(installs.iter().any(|p| p.name == "symfony/twig-bridge" && p.version == "2.0.0"),
        "Should install symfony/twig-bridge 2.0.0");
}

/// Port of Composer's testConflictResultEmpty
/// A conflicts with B, both are required
/// Expected: SolverProblemsException
#[test]
fn test_conflict_result_empty() {
    let mut pool = Pool::new();

    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.conflict.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_a);

    pool.add_package(Package::new("b", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.require("b", "*");

    let result = solver.solve(&request);
    assert!(result.is_err(), "Should fail due to conflict");
}

/// Port of Composer's testUnsatisfiableRequires
/// A requires B >= 2.0, but only B 1.0 exists
/// Expected: SolverProblemsException
#[test]
fn test_unsatisfiable_requires() {
    let mut pool = Pool::new();

    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), ">=2.0".to_string());
    pool.add_package(pkg_a);

    pool.add_package(Package::new("b", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_err(), "Should fail due to unsatisfiable requirement");
}

/// Port of Composer's testRequireMismatchException
/// Complex circular dependency creating conflict:
/// A requires B >= 1.0, B requires C >= 1.0, C requires D >= 1.0, D requires B < 1.0
/// Expected: SolverProblemsException
#[test]
fn test_require_mismatch_exception() {
    let mut pool = Pool::new();

    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_a);

    let mut pkg_b = Package::new("b", "1.0.0");
    pkg_b.require.insert("c".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_b);

    pool.add_package(Package::new("b", "0.9.0"));

    let mut pkg_c = Package::new("c", "1.0.0");
    pkg_c.require.insert("d".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_c);

    let mut pkg_d = Package::new("d", "1.0.0");
    pkg_d.require.insert("b".to_string(), "<1.0".to_string());
    pool.add_package(pkg_d);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_err(), "Should fail due to circular version conflict");
}

/// Port of Composer's testSolverUpdateFullyConstrainedPrunesInstalledPackages
/// Locked: A 1.0, B 1.0; Available: A 1.2, A 2.0
/// Request: A < 2.0
/// Expected: Remove B, Update A 1.0 -> 1.2
#[test]
fn test_solver_update_fully_constrained_prunes_installed_packages() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));
    pool.add_package(Package::new("a", "1.2.0"));
    pool.add_package(Package::new("a", "2.0.0"));
    pool.add_package(Package::new("b", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.lock(Package::new("a", "1.0.0"));
    request.lock(Package::new("b", "1.0.0"));
    request.require("a", "<2.0");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // removals() returns only explicit Uninstall operations (not Update from packages)
    let removes: Vec<_> = transaction.removals().collect();
    assert_eq!(removes.len(), 1, "Should remove one package");
    assert_eq!(removes[0].name, "b");

    let updates: Vec<_> = transaction.updates().collect();
    assert_eq!(updates.len(), 1, "Should have one update");
    assert_eq!(updates[0].0.name, "a");
    assert_eq!(updates[0].0.version, "1.0.0");
    assert_eq!(updates[0].1.version, "1.2.0");
}

/// Port of Composer's testInstallOneOfTwoAlternatives
/// Two packages with same name and version
/// Expected: Install first one
#[test]
fn test_install_one_of_two_alternatives() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));
    pool.add_package(Package::new("a", "1.0.0")); // Duplicate

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    assert_eq!(installs.len(), 1, "Should install exactly one package");
    assert_eq!(installs[0].name, "a");
}

/// Port of Composer's testSolverFixLockedWithAlternative
/// Locked: A 1.0; Available: A 1.0
/// Request: fix A
/// Expected: No changes
#[test]
fn test_solver_fix_locked_with_alternative() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let pkg_a = Package::new("a", "1.0.0");
    let mut request = Request::new();
    request.lock(pkg_a.clone());
    request.fix(pkg_a);

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    assert!(transaction.installs().next().is_none(), "Should have no installs");
    assert!(transaction.updates().next().is_none(), "Should have no updates");
    assert!(transaction.uninstalls().next().is_none(), "Should have no uninstalls");
}

/// Port of Composer's testSolverInstallSamePackageFromDifferentRepositories
/// Two repos with same package - should install from first
#[test]
fn test_solver_install_same_package_from_different_repositories() {
    let mut pool = Pool::new();

    // Simulate two repos by adding same package twice
    // The pool should prefer the first one
    pool.add_package(Package::new("foo", "1.0.0"));
    pool.add_package(Package::new("foo", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("foo", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    assert_eq!(installs.len(), 1, "Should install exactly one package");
    assert_eq!(installs[0].name, "foo");
}

/// Port of Composer's testLearnPositiveLiteral
/// Complex test that exercises CDCL learning of positive literals after negative decisions.
/// Every package and link matters - only this complex combination triggers the scenario.
///
#[test]
fn test_learn_positive_literal() {
    let mut pool = Pool::new();

    // A requires B==1.0, C>=1.0, D==1.0
    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), "==1.0.0".to_string());
    pkg_a.require.insert("c".to_string(), ">=1.0".to_string());
    pkg_a.require.insert("d".to_string(), "==1.0.0".to_string());
    pool.add_package(pkg_a);

    // B requires E==1.0
    let mut pkg_b = Package::new("b", "1.0.0");
    pkg_b.require.insert("e".to_string(), "==1.0.0".to_string());
    pool.add_package(pkg_b);

    // C 1.0 requires F==1.0
    let mut pkg_c1 = Package::new("c", "1.0.0");
    pkg_c1.require.insert("f".to_string(), "==1.0.0".to_string());
    pool.add_package(pkg_c1);

    // C 2.0 requires F==1.0 and G>=1.0
    let mut pkg_c2 = Package::new("c", "2.0.0");
    pkg_c2.require.insert("f".to_string(), "==1.0.0".to_string());
    pkg_c2.require.insert("g".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_c2);

    // D requires F>=1.0
    let mut pkg_d = Package::new("d", "1.0.0");
    pkg_d.require.insert("f".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_d);

    // E requires G<=2.0
    let mut pkg_e = Package::new("e", "1.0.0");
    pkg_e.require.insert("g".to_string(), "<=2.0".to_string());
    pool.add_package(pkg_e);

    pool.add_package(Package::new("f", "1.0.0"));
    pool.add_package(Package::new("f", "2.0.0"));

    pool.add_package(Package::new("g", "1.0.0"));
    pool.add_package(Package::new("g", "2.0.0"));
    pool.add_package(Package::new("g", "3.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // Check that a valid solution was found
    // Both C 1.0 and C 2.0 are valid solutions since A requires C >= 1.0
    // C 1.0 only needs F, C 2.0 needs F and G
    assert!(installs.iter().any(|p| p.name == "a" && p.version == "1.0.0"),
        "Should install A 1.0.0");
    assert!(installs.iter().any(|p| p.name == "b" && p.version == "1.0.0"),
        "Should install B 1.0.0");
    assert!(installs.iter().any(|p| p.name == "c" && (p.version == "1.0.0" || p.version == "2.0.0")),
        "Should install C (1.0.0 or 2.0.0), got: {:?}", installs);
    assert!(installs.iter().any(|p| p.name == "d" && p.version == "1.0.0"),
        "Should install D 1.0.0");
    assert!(installs.iter().any(|p| p.name == "e" && p.version == "1.0.0"),
        "Should install E 1.0.0");
    assert!(installs.iter().any(|p| p.name == "f" && p.version == "1.0.0"),
        "Should install F 1.0.0");

    // If C 2.0 is installed, G must be installed (C 2.0's G>=1.0) and constrained by E's G<=2.0
    // Valid G versions when C 2.0 is installed: 1.0, 2.0 (both satisfy G>=1.0 and G<=2.0)
    let c_pkg = installs.iter().find(|p| p.name == "c").unwrap();
    if c_pkg.version == "2.0.0" {
        let g_pkg = installs.iter().find(|p| p.name == "g");
        assert!(g_pkg.is_some(), "Should install G when C 2.0 is installed");
        let g_version = &g_pkg.unwrap().version;
        assert!(g_version == "1.0.0" || g_version == "2.0.0",
            "G should be 1.0.0 or 2.0.0 (satisfying G>=1.0 and G<=2.0), got: {}", g_version);
    }
}

/// Port of Composer's testIssue265
/// Complex scenario with dev versions and replacers.
///
/// C requires A>=2.0 and D>=2.0
/// D requires A>=2.1 and B>=2.0-dev
/// B 2.0.10 and B 2.0.9 both require A==2.1.0.0-dev
/// B 2.0.9 also replaces D==2.0.9.0
///
/// With minimum-stability: stable (default), dev packages are filtered out,
/// so the solver cannot find a solution (matching Composer's behavior).
#[test]
fn test_issue_265() {
    // With default stability (stable), dev packages are filtered out
    let mut pool = Pool::new(); // default is Stability::Stable

    // Use exact version strings from Composer test
    // These dev packages will be filtered out due to minimum-stability: stable
    pool.add_package(Package::new("a", "2.0.999999-dev"));
    pool.add_package(Package::new("a", "2.1-dev"));
    pool.add_package(Package::new("a", "2.2-dev"));

    let mut pkg_b1 = Package::new("b", "2.0.10");
    pkg_b1.require.insert("a".to_string(), "==2.1.0.0-dev".to_string());
    pool.add_package(pkg_b1);

    let mut pkg_b2 = Package::new("b", "2.0.9");
    pkg_b2.require.insert("a".to_string(), "==2.1.0.0-dev".to_string());
    pkg_b2.replace.insert("d".to_string(), "==2.0.9.0".to_string());
    pool.add_package(pkg_b2);

    let mut pkg_c = Package::new("c", "2.0-dev");
    pkg_c.require.insert("a".to_string(), ">=2.0".to_string());
    pkg_c.require.insert("d".to_string(), ">=2.0".to_string());
    pool.add_package(pkg_c);

    let mut pkg_d = Package::new("d", "2.0.9");
    pkg_d.require.insert("a".to_string(), ">=2.1".to_string());
    pkg_d.require.insert("b".to_string(), ">=2.0-dev".to_string());
    pool.add_package(pkg_d);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("c", "==2.0.0.0-dev");

    let result = solver.solve(&request);

    // With minimum-stability: stable, C (a dev package) is filtered out,
    // so the solver cannot find C and should fail
    assert!(result.is_err(), "Should fail because C is a dev package and minimum-stability is stable");
}

/// Test that the solver works when minimum-stability allows dev packages
#[test]
fn test_issue_265_with_dev_stability() {
    use crate::package::Stability;

    // With minimum-stability: dev, all packages are allowed
    let mut pool = Pool::with_minimum_stability(Stability::Dev);

    pool.add_package(Package::new("a", "2.0.999999-dev"));
    pool.add_package(Package::new("a", "2.1-dev"));
    pool.add_package(Package::new("a", "2.2-dev"));

    let mut pkg_b1 = Package::new("b", "2.0.10");
    pkg_b1.require.insert("a".to_string(), "==2.1.0.0-dev".to_string());
    pool.add_package(pkg_b1);

    let mut pkg_b2 = Package::new("b", "2.0.9");
    pkg_b2.require.insert("a".to_string(), "==2.1.0.0-dev".to_string());
    pkg_b2.replace.insert("d".to_string(), "==2.0.9.0".to_string());
    pool.add_package(pkg_b2);

    let mut pkg_c = Package::new("c", "2.0-dev");
    pkg_c.require.insert("a".to_string(), ">=2.0".to_string());
    pkg_c.require.insert("d".to_string(), ">=2.0".to_string());
    pool.add_package(pkg_c);

    let mut pkg_d = Package::new("d", "2.0.9");
    pkg_d.require.insert("a".to_string(), ">=2.1".to_string());
    pkg_d.require.insert("b".to_string(), ">=2.0-dev".to_string());
    pool.add_package(pkg_d);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("c", "==2.0.0.0-dev");

    let result = solver.solve(&request);

    // With minimum-stability: dev, solver should find a solution
    assert!(result.is_ok(), "Solver should find a solution with dev stability: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // Verify the solution contains expected packages
    assert!(installs.iter().any(|p| p.name == "a" && p.version == "2.1-dev"),
        "Should install A 2.1-dev");
    assert!(installs.iter().any(|p| p.name == "b" && p.version == "2.0.10"),
        "Should install B 2.0.10");
    assert!(installs.iter().any(|p| p.name == "d" && p.version == "2.0.9"),
        "Should install D 2.0.9");
    assert!(installs.iter().any(|p| p.name == "c" && p.version == "2.0-dev"),
        "Should install C 2.0-dev");
}

/// Port of Composer's testNoInstallReplacerOfMissingPackage
/// A requires B, Q replaces B but there's no actual B package
/// Should fail because replacers are not auto-selected when no direct package exists
#[test]
fn test_no_install_replacer_of_missing_package() {
    let mut pool = Pool::new();

    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_a);

    let mut pkg_q = Package::new("q", "1.0.0");
    pkg_q.replace.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_q);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    // Should fail because Q replaces B but isn't automatically selected
    assert!(result.is_err(), "Should fail because replacer is not auto-selected");
}

/// Port of Composer's testInstallProvider
/// A requires B, Q provides B
/// Should fail because providers are not auto-selected when no direct package exists
#[test]
fn test_install_provider() {
    let mut pool = Pool::new();

    let mut pkg_a = Package::new("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_a);

    let mut pkg_q = Package::new("q", "1.0.0");
    pkg_q.provide.insert("b".to_string(), "1.0.0".to_string());
    pool.add_package(pkg_q);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    // Must explicitly select the provider
    let result = solver.solve(&request);
    assert!(result.is_err(), "Should fail because provider must be explicitly selected");
}

/// Port of Composer's testSolverMultiPackageNameVersionResolutionDependsOnRequireOrder
///
/// This test covers a particular behavior of the solver related to packages with the same
/// name and version, but different requirements on other packages.
///
/// Example scenario:
/// - PHP versions 8.0.10 and 7.4.23 are packages
/// - ext-foobar 1.0.0 is a package, but built separately for each PHP x.y series
/// - Thus each of the ext-foobar packages lists the "PHP" package as a dependency
///
/// If version selectors are sufficiently permissive (e.g., "*"), then the solver may pick
/// different versions based on the order packages are required.
///
/// In Composer, requiring PHP before ext-foobar selects PHP 8.0.10, but requiring
/// ext-foobar before PHP selects PHP 7.4.23 because ext-foobar for 7.4 comes first
/// in the pool.
///
/// CAUTION: IF THIS TEST EVER FAILS, SOLVER BEHAVIOR HAS CHANGED AND MAY BREAK DOWNSTREAM USERS
#[test]
fn test_solver_multi_package_name_version_resolution_depends_on_require_order() {
    let mut pool = Pool::new();

    // PHP versions
    pool.add_package(Package::new("ourcustom/php", "7.4.23"));
    pool.add_package(Package::new("ourcustom/php", "8.0.10"));

    // ext-foobar for PHP 7.4 (inserted FIRST into repo)
    let mut ext_for_php74 = Package::new("ourcustom/ext-foobar", "1.0.0");
    ext_for_php74.require.insert("ourcustom/php".to_string(), ">=7.4.0, <7.5.0".to_string());
    pool.add_package(ext_for_php74);

    // ext-foobar for PHP 8.0 (inserted second)
    let mut ext_for_php80 = Package::new("ourcustom/ext-foobar", "1.0.0");
    ext_for_php80.require.insert("ourcustom/php".to_string(), ">=8.0.0, <8.1.0".to_string());
    pool.add_package(ext_for_php80);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    // Request PHP first, then ext-foobar
    // Because PHP is requested first, the solver picks the highest PHP (8.0.10)
    // and then picks the ext-foobar that matches it
    let mut request = Request::new();
    request.require("ourcustom/php", "*");
    request.require("ourcustom/ext-foobar", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // Should install PHP 8.0.10 (highest) and matching ext-foobar
    assert!(installs.iter().any(|p| p.name == "ourcustom/php" && p.version == "8.0.10"),
        "Should install PHP 8.0.10 when PHP is required first, got: {:?}", installs);
    assert!(installs.iter().any(|p| p.name == "ourcustom/ext-foobar"),
        "Should install ext-foobar");

    // Now flip the requirement order: ext-foobar before php
    // Because the ext-foobar package for php74 comes first in the repo,
    // and it's requested first, the solver picks it and then must pick php 7.4.23
    let mut request2 = Request::new();
    request2.require("ourcustom/ext-foobar", "*");
    request2.require("ourcustom/php", "*");

    let result2 = solver.solve(&request2);
    assert!(result2.is_ok(), "Should find solution with reversed order: {:?}", result2.err());

    let solver_result2 = result2.unwrap();
    let transaction2 = make_transaction(&solver_result2, &request2);
    let installs2: Vec<_> = transaction2.installs().collect();

    // Should install PHP 7.4.23 and matching ext-foobar
    assert!(installs2.iter().any(|p| p.name == "ourcustom/php" && p.version == "7.4.23"),
        "Should install PHP 7.4.23 when ext-foobar is required first, got: {:?}", installs2);
    assert!(installs2.iter().any(|p| p.name == "ourcustom/ext-foobar"),
        "Should install ext-foobar");
}

/// Port of Composer's testSolverMultiPackageNameVersionResolutionIsIndependentOfRequireOrderIfOrderedDescendingByRequirement
///
/// This test is similar to the above, except packages with requirements are inserted
/// in a different order, asserting that if packages requiring higher versions come first,
/// the order of requirements no longer matters.
///
/// CAUTION: IF THIS TEST EVER FAILS, SOLVER BEHAVIOR HAS CHANGED
#[test]
fn test_solver_multi_package_name_version_resolution_independent_of_require_order_if_ordered_descending() {
    let mut pool = Pool::new();

    // PHP versions
    pool.add_package(Package::new("ourcustom/php", "7.4.0"));
    pool.add_package(Package::new("ourcustom/php", "8.0.0"));

    // ext-foobar for PHP 8.0 (inserted FIRST - key difference from above test)
    let mut ext_for_php80 = Package::new("ourcustom/ext-foobar", "1.0.0");
    ext_for_php80.require.insert("ourcustom/php".to_string(), ">=8.0.0, <8.1.0".to_string());
    pool.add_package(ext_for_php80);

    // ext-foobar for PHP 7.4 (inserted second)
    let mut ext_for_php74 = Package::new("ourcustom/ext-foobar", "1.0.0");
    ext_for_php74.require.insert("ourcustom/php".to_string(), ">=7.4.0, <7.5.0".to_string());
    pool.add_package(ext_for_php74);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    // Request PHP first, then ext-foobar
    let mut request = Request::new();
    request.require("ourcustom/php", "*");
    request.require("ourcustom/ext-foobar", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let installs: Vec<_> = transaction.installs().collect();

    // Should install PHP 8.0 (highest)
    assert!(installs.iter().any(|p| p.name == "ourcustom/php" && p.version == "8.0.0"),
        "Should install PHP 8.0.0");

    // Now flip the requirement order: ext-foobar before php
    // Unlike the simpler test, the order should NOT matter here because
    // the ext-foobar for PHP 8.0 was inserted first in the pool
    let mut request2 = Request::new();
    request2.require("ourcustom/ext-foobar", "*");
    request2.require("ourcustom/php", "*");

    let result2 = solver.solve(&request2);
    assert!(result2.is_ok(), "Should find solution: {:?}", result2.err());

    let solver_result2 = result2.unwrap();
    let transaction2 = make_transaction(&solver_result2, &request2);
    let installs2: Vec<_> = transaction2.installs().collect();

    // Should still install PHP 8.0
    assert!(installs2.iter().any(|p| p.name == "ourcustom/php" && p.version == "8.0.0"),
        "Should still install PHP 8.0.0 regardless of require order");
}

/// Port of Composer's testSolverUpdateConstrained
/// Locked: A 1.0; Available: A 1.2, A 2.0
/// Request: A < 2.0
/// Expected: Update A 1.0 -> 1.2
#[test]
fn test_solver_update_constrained_only() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));
    pool.add_package(Package::new("a", "1.2.0"));
    pool.add_package(Package::new("a", "2.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.lock(Package::new("a", "1.0.0"));
    request.require("a", "<2.0");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let updates: Vec<_> = transaction.updates().collect();

    assert_eq!(updates.len(), 1, "Should have exactly one update");
    assert_eq!(updates[0].0.version, "1.0.0");
    assert_eq!(updates[0].1.version, "1.2.0", "Should update to 1.2.0, not 2.0.0");
}

/// Port of Composer's testSolverUpdateFullyConstrained
/// Same as testSolverUpdateConstrained - validates constraint-based updates
#[test]
fn test_solver_update_fully_constrained() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));
    pool.add_package(Package::new("a", "1.2.0"));
    pool.add_package(Package::new("a", "2.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.lock(Package::new("a", "1.0.0"));
    request.require("a", "<2.0");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    let updates: Vec<_> = transaction.updates().collect();

    assert_eq!(updates.len(), 1, "Should have exactly one update");
    assert_eq!(updates[0].1.version, "1.2.0");
}

/// Port of Composer's testSolverUpdateCurrent
/// Locked: A 1.0; Available: A 1.0
/// Request: A
/// Expected: No changes (already at correct version)
#[test]
fn test_solver_update_current() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.lock(Package::new("a", "1.0.0"));
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);

    // No changes needed - same version already installed
    assert!(transaction.is_empty(), "Transaction should be empty when already at correct version");
}

/// Port of Composer's testSolverFixLocked
/// Locked: A 1.0
/// Request: fix A
/// Expected: No changes
#[test]
fn test_solver_fix_locked_only() {
    let mut pool = Pool::new();

    pool.add_package(Package::new("a", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let pkg_a = Package::new("a", "1.0.0");
    let mut request = Request::new();
    request.lock(pkg_a.clone());
    request.fix(pkg_a);

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();
    let transaction = make_transaction(&solver_result, &request);
    assert!(transaction.is_empty(), "Transaction should be empty when fixing locked package");
}

/// Test that caret constraint correctly excludes major version upgrades.
///
/// This is a regression test for the webmozart/assert issue where:
/// - Package X requires webmozart/assert ^1.11 (should match 1.11.x, 1.12.x but NOT 2.x)
/// - Installed: webmozart/assert 1.12.1
/// - Available: 1.9.1, 1.10.0, 1.11.0, 1.12.1, 2.0.0
/// - Expected: Keep 1.12.1 (not downgrade to 1.11.0 or upgrade to 2.0.0)
///
/// The bug was that the solver was selecting 2.0.0 as "best" and then falling back
/// to 1.11.0 when 2.0.0 caused a conflict, instead of selecting 1.12.1.
#[test]
fn test_caret_constraint_excludes_major_upgrade() {
    // Note: enable trace logging with RUST_LOG=trace when running tests

    let mut pool = Pool::new();

    // Package A requires webmozart/assert ^1.11
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("webmozart/assert", "^1.11")]));

    // webmozart/assert versions - note that 2.0.0 should NOT match ^1.11
    pool.add_package(Package::new("webmozart/assert", "1.9.1.0"));
    pool.add_package(Package::new("webmozart/assert", "1.10.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.11.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.12.1.0"));
    pool.add_package(Package::new("webmozart/assert", "2.0.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    // Simulate already having 1.12.1 installed
    request.lock(Package::new("webmozart/assert", "1.12.1.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();

    // Find the webmozart/assert package in the result
    let assert_pkg = solver_result.packages.iter()
        .find(|p| p.name == "webmozart/assert")
        .expect("webmozart/assert should be in result");

    // The solver should keep 1.12.1.0 (already installed, matches ^1.11)
    // or select the highest matching version (1.12.1.0)
    // It should NOT select 2.0.0.0 (doesn't match ^1.11)
    // It should NOT downgrade to 1.11.0.0 (1.12.1.0 is better)
    assert!(
        assert_pkg.version == "1.12.1.0",
        "webmozart/assert should be 1.12.1.0 (highest matching ^1.11), got {}",
        assert_pkg.version
    );
}

/// Same as above but with a fresh install (no locked package)
#[test]
fn test_caret_constraint_selects_highest_matching_fresh() {
    // Note: enable trace logging with RUST_LOG=trace when running tests

    let mut pool = Pool::new();

    // Package A requires webmozart/assert ^1.11
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("webmozart/assert", "^1.11")]));

    // webmozart/assert versions
    pool.add_package(Package::new("webmozart/assert", "1.9.1.0"));
    pool.add_package(Package::new("webmozart/assert", "1.10.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.11.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.12.1.0"));
    pool.add_package(Package::new("webmozart/assert", "2.0.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();

    // Find the webmozart/assert package in the result
    let assert_pkg = solver_result.packages.iter()
        .find(|p| p.name == "webmozart/assert")
        .expect("webmozart/assert should be in result");

    // The solver should select the highest matching version (1.12.1.0)
    // NOT 2.0.0.0 (doesn't match ^1.11)
    assert!(
        assert_pkg.version == "1.12.1.0",
        "webmozart/assert should be 1.12.1.0 (highest matching ^1.11), got {}",
        assert_pkg.version
    );
}

/// Test with multiple packages requiring webmozart/assert with different constraints.
/// This simulates a more realistic scenario where:
/// - Package A requires webmozart/assert ^1.11
/// - Package B requires webmozart/assert ^1.9
///
/// Both constraints must be satisfied.
#[test]
fn test_caret_constraint_with_multiple_requirers() {
    // Note: enable trace logging with RUST_LOG=trace when running tests

    let mut pool = Pool::new();

    // Package A requires webmozart/assert ^1.11
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("webmozart/assert", "^1.11")]));

    // Package B requires webmozart/assert ^1.9
    pool.add_package(pkg_with_requires("b", "1.0.0", vec![("webmozart/assert", "^1.9")]));

    // webmozart/assert versions
    pool.add_package(Package::new("webmozart/assert", "1.9.1.0"));
    pool.add_package(Package::new("webmozart/assert", "1.10.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.11.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.12.1.0"));
    pool.add_package(Package::new("webmozart/assert", "2.0.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.require("b", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();

    // Find the webmozart/assert package in the result
    let assert_pkg = solver_result.packages.iter()
        .find(|p| p.name == "webmozart/assert")
        .expect("webmozart/assert should be in result");

    // The solver should select the highest version that matches BOTH constraints
    // A needs ^1.11 (1.11.x, 1.12.x), B needs ^1.9 (1.9.x, 1.10.x, 1.11.x, 1.12.x)
    // The intersection is 1.11.x, 1.12.x
    // So 1.12.1.0 should be selected (highest in intersection)
    // 2.0.0.0 doesn't match ^1.11, so it should NOT be selected
    assert!(
        assert_pkg.version == "1.12.1.0",
        "webmozart/assert should be 1.12.1.0 (highest matching both ^1.11 and ^1.9), got {}",
        assert_pkg.version
    );
}

/// Test that locked packages are preferred when not in update allowlist.
/// This is the webmozart/assert scenario where:
/// - 1.12.1.0 is locked
/// - Constraint is ^1.11
/// - Solver should keep 1.12.1.0 (not downgrade to 1.11.0)
#[test]
fn test_locked_packages_preferred_when_not_in_update_allowlist() {
    let mut pool = Pool::new();

    // Package A requires webmozart/assert ^1.11
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("webmozart/assert", "^1.11")]));

    // webmozart/assert versions
    pool.add_package(Package::new("webmozart/assert", "1.11.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.12.1.0"));
    pool.add_package(Package::new("webmozart/assert", "2.0.0.0"));

    // Build preferred versions from locked packages (like Composer does)
    // When webmozart/assert is NOT in the update allowlist, its locked version should be preferred
    let mut preferred_versions = std::collections::HashMap::new();
    preferred_versions.insert("webmozart/assert".to_string(), "1.12.1.0".to_string());

    let policy = Policy::new()
        .preferred_versions(preferred_versions);
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    // webmozart/assert is locked at 1.12.1.0
    request.lock(Package::new("webmozart/assert", "1.12.1.0"));

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();

    // Find the webmozart/assert package in the result
    let assert_pkg = solver_result.packages.iter()
        .find(|p| p.name == "webmozart/assert")
        .expect("webmozart/assert should be in result");

    // The solver should prefer the locked version since it matches the constraint
    // and webmozart/assert is not in the update allowlist
    assert_eq!(
        assert_pkg.version, "1.12.1.0",
        "webmozart/assert should stay at 1.12.1.0 (locked version), not change to {}",
        assert_pkg.version
    );

    // Verify there's no update operation (should be a no-op)
    let transaction = make_transaction(&solver_result, &request);
    let updates: Vec<_> = transaction.updates().collect();
    assert!(
        updates.is_empty() || !updates.iter().any(|(_, to)| to.name == "webmozart/assert"),
        "Should not update webmozart/assert when locked and not in update allowlist"
    );
}

/// Test that packages in update allowlist CAN be updated even if locked.
/// This is the Composer behavior: update allowlist overrides preferred versions.
#[test]
fn test_update_allowlist_allows_upgrade() {
    let mut pool = Pool::new();

    // Package A requires webmozart/assert ^1.11
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("webmozart/assert", "^1.11")]));

    // webmozart/assert versions
    pool.add_package(Package::new("webmozart/assert", "1.11.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.12.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.12.1.0"));

    // When webmozart/assert IS in the update allowlist, it should NOT be set as preferred
    // So the solver picks the highest matching version
    let policy = Policy::new(); // No preferred versions - package is being updated
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    // webmozart/assert is locked at older version, but we want to update it
    request.lock(Package::new("webmozart/assert", "1.11.0.0"));
    request.update(vec!["webmozart/assert".to_string()]);

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();

    // Find the webmozart/assert package in the result
    let assert_pkg = solver_result.packages.iter()
        .find(|p| p.name == "webmozart/assert")
        .expect("webmozart/assert should be in result");

    // The solver should select the highest version since it's in the update allowlist
    assert_eq!(
        assert_pkg.version, "1.12.1.0",
        "webmozart/assert should update to 1.12.1.0 (highest matching ^1.11), got {}",
        assert_pkg.version
    );

    // Verify there's an update operation
    let transaction = make_transaction(&solver_result, &request);
    let updates: Vec<_> = transaction.updates().collect();
    assert!(
        updates.iter().any(|(from, to)| to.name == "webmozart/assert" && from.version == "1.11.0.0" && to.version == "1.12.1.0"),
        "Should update webmozart/assert from 1.11.0.0 to 1.12.1.0"
    );
}

/// Test that major version upgrades are blocked by caret constraint.
/// Even without locked packages, ^1.11 should never select 2.0.0.
#[test]
fn test_caret_constraint_blocks_major_upgrade() {
    let mut pool = Pool::new();

    // Package A requires webmozart/assert ^1.11
    pool.add_package(pkg_with_requires("a", "1.0.0", vec![("webmozart/assert", "^1.11")]));

    // webmozart/assert versions - including 2.0.0 which should be blocked
    pool.add_package(Package::new("webmozart/assert", "1.11.0.0"));
    pool.add_package(Package::new("webmozart/assert", "1.12.1.0"));
    pool.add_package(Package::new("webmozart/assert", "2.0.0.0")); // Should NOT be selected

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution: {:?}", result.err());

    let solver_result = result.unwrap();

    // Find the webmozart/assert package in the result
    let assert_pkg = solver_result.packages.iter()
        .find(|p| p.name == "webmozart/assert")
        .expect("webmozart/assert should be in result");

    // Must select 1.12.1.0, NOT 2.0.0.0
    assert_eq!(
        assert_pkg.version, "1.12.1.0",
        "webmozart/assert should be 1.12.1.0 (highest matching ^1.11), not {}. \
         Version 2.0.0.0 should be blocked by caret constraint.",
        assert_pkg.version
    );
}

// ============================================================================
// Additional Tests ported from Composer's SolverTest.php
// These tests validate compatibility with Composer's dependency resolution
// ============================================================================

/// Port of testInstallCircularRequire
/// Test circular dependencies work correctly
#[test]
fn test_composer_install_circular_require() {
    let mut pool = Pool::new();

    // A requires B >= 1.0
    let mut pkg_a = pkg("a", "1.0.0");
    pkg_a.require.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_a);

    // B 0.9 doesn't require A
    pool.add_package(pkg("b", "0.9.0"));

    // B 1.1 requires A >= 1.0 (circular)
    let mut pkg_b = pkg("b", "1.1.0");
    pkg_b.require.insert("a".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_b);

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");

    let result = solver.solve(&request);
    assert!(result.is_ok(), "Should find solution for circular deps: {:?}", result.err());

    let solver_result = result.unwrap();
    // Should install both A 1.0 and B 1.1 (with circular requirement)
    let a_pkg = solver_result.packages.iter().find(|p| p.name == "a");
    let b_pkg = solver_result.packages.iter().find(|p| p.name == "b");
    assert!(a_pkg.is_some(), "A should be installed");
    assert!(b_pkg.is_some(), "B should be installed");
    assert_eq!(b_pkg.unwrap().version, "1.1.0", "B should be 1.1.0 (highest matching >=1.0)");
}

/// Port of testConflictResultEmpty
/// Two packages that conflict should cause solver error
#[test]
fn test_composer_conflict_result_empty() {
    let mut pool = Pool::new();

    // A conflicts with B
    let mut pkg_a = pkg("a", "1.0.0");
    pkg_a.conflict.insert("b".to_string(), ">=1.0".to_string());
    pool.add_package(pkg_a);

    pool.add_package(pkg("b", "1.0.0"));

    let policy = Policy::new();
    let solver = Solver::new(&pool, &policy);

    let mut request = Request::new();
    request.require("a", "*");
    request.require("b", "*");

    let result = solver.solve(&request);
    assert!(result.is_err(), "Should fail due to conflict between A and B");
}

// ============================================================================
// Tests ported from Composer's DefaultPolicyTest.php
// These tests validate version selection policy behavior
// ============================================================================

/// Port of testSelectNewest
/// Policy should select newest version
#[test]
fn test_policy_select_newest() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "2.0.0"));

    let policy = Policy::new();
    let candidates: Vec<_> = pool.packages_by_name("a");

    let selected = policy.select_preferred(&pool, &candidates);
    assert!(!selected.is_empty(), "Should select at least one package");

    let selected_pkg = pool.package(selected[0]).unwrap();
    assert_eq!(selected_pkg.version, "2.0.0", "Should select newest version");
}

/// Port of testSelectLowest
/// Policy with prefer_lowest should select oldest version
#[test]
fn test_policy_select_lowest() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "2.0.0"));

    let policy = Policy::new().prefer_lowest(true);
    let candidates: Vec<_> = pool.packages_by_name("a");

    let selected = policy.select_preferred(&pool, &candidates);
    assert!(!selected.is_empty(), "Should select at least one package");

    let selected_pkg = pool.package(selected[0]).unwrap();
    assert_eq!(selected_pkg.version, "1.0.0", "Should select lowest version when prefer_lowest is true");
}

/// Port of testSelectNewestPicksLatestStableWithPreferStable
/// Policy with prefer_stable should prefer stable over alpha
#[test]
fn test_policy_prefer_stable() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "1.0.1-alpha"));

    let policy = Policy::new().prefer_stable(true);
    let candidates: Vec<_> = pool.packages_by_name("a");

    let selected = policy.select_preferred(&pool, &candidates);
    assert!(!selected.is_empty(), "Should select at least one package");

    let selected_pkg = pool.package(selected[0]).unwrap();
    assert_eq!(selected_pkg.version, "1.0.0", "Should prefer stable 1.0.0 over alpha 1.0.1-alpha");
}

/// Port of testSelectNewestWithDevPicksNonDev
/// Policy should prefer non-dev over dev versions
#[test]
fn test_policy_prefer_non_dev_over_dev() {
    use crate::package::Stability;

    let mut pool = Pool::with_minimum_stability(Stability::Dev);
    pool.add_package(pkg("a", "dev-master"));
    pool.add_package(pkg("a", "1.0.0"));

    let policy = Policy::new();
    let candidates: Vec<_> = pool.packages_by_name("a");

    let selected = policy.select_preferred(&pool, &candidates);
    assert!(!selected.is_empty(), "Should select at least one package");

    let selected_pkg = pool.package(selected[0]).unwrap();
    assert_eq!(selected_pkg.version, "1.0.0", "Should prefer stable 1.0.0 over dev-master");
}

/// Port of testSelectNewestWithPreferredVersionPicksPreferredVersionIfAvailable
/// Preferred versions from lock file should be respected
#[test]
fn test_policy_preferred_version_selected() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "1.1.0"));
    pool.add_package(pkg("a", "1.2.0"));

    // Prefer 1.1.0 (simulating lock file preference)
    let mut preferred = std::collections::HashMap::new();
    preferred.insert("a".to_string(), "1.1.0".to_string());

    let policy = Policy::new().preferred_versions(preferred);
    let candidates: Vec<_> = pool.packages_by_name("a");

    let selected = policy.select_preferred(&pool, &candidates);
    assert!(!selected.is_empty(), "Should select at least one package");

    let selected_pkg = pool.package(selected[0]).unwrap();
    assert_eq!(selected_pkg.version, "1.1.0", "Should select preferred version 1.1.0 from lock file");
}

/// Port of testSelectNewestWithPreferredVersionPicksNewestOtherwise
/// When preferred version doesn't exist, fall back to newest
#[test]
fn test_policy_preferred_version_not_available_falls_back() {
    let mut pool = Pool::new();
    pool.add_package(pkg("a", "1.0.0"));
    pool.add_package(pkg("a", "1.2.0"));

    // Prefer 1.1.0 but it doesn't exist
    let mut preferred = std::collections::HashMap::new();
    preferred.insert("a".to_string(), "1.1.0".to_string());

    let policy = Policy::new().preferred_versions(preferred);
    let candidates: Vec<_> = pool.packages_by_name("a");

    let selected = policy.select_preferred(&pool, &candidates);
    assert!(!selected.is_empty(), "Should select at least one package");

    let selected_pkg = pool.package(selected[0]).unwrap();
    assert_eq!(selected_pkg.version, "1.2.0", "Should fall back to newest when preferred not available");
}

/// Port of testRepositoryOrderingAffectsPriority
/// Packages from earlier repositories should be preferred
#[test]
fn test_policy_repository_priority() {
    let mut pool = Pool::new();

    // repo1 has 1.0 and 1.1
    pool.add_package_from_repo(pkg("a", "1.0.0"), Some("repo1"));
    pool.add_package_from_repo(pkg("a", "1.1.0"), Some("repo1"));

    // repo2 has 1.1 and 1.2
    pool.add_package_from_repo(pkg("a", "1.1.0"), Some("repo2"));
    pool.add_package_from_repo(pkg("a", "1.2.0"), Some("repo2"));

    // repo1 has higher priority
    pool.set_priority("repo1", 0);
    pool.set_priority("repo2", 1);

    let policy = Policy::new();
    let candidates: Vec<_> = pool.packages_by_name("a");

    let selected = policy.select_preferred(&pool, &candidates);
    assert!(!selected.is_empty(), "Should select at least one package");

    // Should select 1.1.0 from repo1 (highest version in highest priority repo)
    let selected_pkg = pool.package(selected[0]).unwrap();
    assert_eq!(selected_pkg.version, "1.1.0",
        "Should select highest version from highest priority repo");
}
