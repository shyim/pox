//! Demand-driven pool builder for efficient package loading.
//!
//! This module implements a pool builder that only loads packages that are
//! reachable from root requirements, similar to PHP Composer's PoolBuilder.
//! This dramatically reduces the pool size compared to loading all packages upfront.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use phpx_semver::{Constraint, ConstraintInterface, MultiConstraint, VersionParser};

use super::pool::{Pool, PackageId};
use super::request::Request;
use crate::package::{Package, AliasPackage};
use crate::repository::Repository;
use crate::util::is_platform_package;

/// Builds a pool by demand-driven loading of packages.
///
/// Instead of loading all packages upfront, this builder:
/// 1. Starts with root requirements
/// 2. Loads packages matching those requirements
/// 3. Recursively loads dependencies of loaded packages
/// 4. Only includes packages that are actually reachable
pub struct PoolBuilder {
    /// Version parser for constraint operations
    version_parser: VersionParser,

    /// Packages marked for loading (name -> constraint)
    packages_to_load: HashMap<String, Box<dyn ConstraintInterface>>,

    /// Packages already loaded (name -> constraint that was loaded)
    loaded_packages: HashMap<String, Box<dyn ConstraintInterface>>,

    /// The packages that have been loaded into the pool
    loaded_package_data: Vec<Arc<Package>>,

    /// Aliases to add to the pool
    aliases: Vec<AliasPackage>,

    /// Root requirements (should not be extended by transitive deps)
    max_extended_reqs: HashSet<String>,

    /// Track which packages we've seen to avoid duplicates
    seen_packages: HashSet<(String, String)>,
}

impl PoolBuilder {
    /// Create a new pool builder.
    pub fn new() -> Self {
        Self {
            version_parser: VersionParser::new(),
            packages_to_load: HashMap::new(),
            loaded_packages: HashMap::new(),
            loaded_package_data: Vec::new(),
            aliases: Vec::new(),
            max_extended_reqs: HashSet::new(),
            seen_packages: HashSet::new(),
        }
    }

    /// Build a pool from repositories using demand-driven loading.
    pub fn build_pool(&mut self, repositories: &[&dyn Repository], request: &Request) -> Pool {
        let start = std::time::Instant::now();

        // Reset state
        self.packages_to_load.clear();
        self.loaded_packages.clear();
        self.loaded_package_data.clear();
        self.aliases.clear();
        self.max_extended_reqs.clear();
        self.seen_packages.clear();

        // Step 1: Mark fixed/locked packages as loaded
        for fixed in &request.fixed_packages {
            let name_lower = fixed.name.to_lowercase();
            // Create a constraint matching exactly this version
            if let Ok(constraint) = self.version_parser.parse_constraints(&format!("={}", fixed.version)) {
                self.loaded_packages.insert(name_lower, constraint);
            }
        }

        for locked in &request.locked_packages {
            let name_lower = locked.name.to_lowercase();
            if let Ok(constraint) = self.version_parser.parse_constraints(&format!("={}", locked.version)) {
                self.loaded_packages.insert(name_lower.clone(), constraint);

                // Also mark replaced packages as loaded (replace = conflict with all versions)
                for (replaced, _) in &locked.replace {
                    let replaced_lower = replaced.to_lowercase();
                    if !self.loaded_packages.contains_key(&replaced_lower) {
                        if let Ok(c) = self.version_parser.parse_constraints("*") {
                            self.loaded_packages.insert(replaced_lower, c);
                        }
                    }
                }
            }
        }

        // Step 2: Mark root requirements for loading
        for (name, constraint_str) in request.all_requires() {
            let name_lower = name.to_lowercase();

            // Skip if already loaded (fixed/locked)
            if self.loaded_packages.contains_key(&name_lower) {
                continue;
            }

            // Skip platform packages
            if is_platform_package(&name_lower) {
                continue;
            }

            if let Ok(constraint) = self.version_parser.parse_constraints(constraint_str) {
                self.packages_to_load.insert(name_lower.clone(), constraint);
                self.max_extended_reqs.insert(name_lower);
            }
        }

        // Step 3: Load packages in waves until nothing left to load
        while !self.packages_to_load.is_empty() {
            self.load_packages_marked_for_loading(repositories);
        }

        log::info!(
            "PoolBuilder loaded {} packages in {:?}",
            self.loaded_package_data.len(),
            start.elapsed()
        );

        // Step 4: Build the pool from loaded packages
        let mut pool = Pool::new();

        for package in &self.loaded_package_data {
            pool.add_package(package.clone(), None);
        }

        for alias in &self.aliases {
            pool.add_alias_package(alias.clone());
        }

        // Add fixed packages to the pool
        for fixed in &request.fixed_packages {
            // Find or create the fixed package
            let existing = self.loaded_package_data.iter()
                .find(|p| p.name.eq_ignore_ascii_case(&fixed.name) && p.version == fixed.version);

            if existing.is_none() {
                // Create a minimal package for the fixed package
                let pkg = Package {
                    name: fixed.name.clone(),
                    version: fixed.version.clone(),
                    ..Default::default()
                };
                pool.add_package(Arc::new(pkg), None);
            }
        }

        // Add locked packages to the pool
        for locked in &request.locked_packages {
            let existing = self.loaded_package_data.iter()
                .find(|p| p.name.eq_ignore_ascii_case(&locked.name) && p.version == locked.version);

            if existing.is_none() {
                pool.add_package(Arc::new(locked.clone()), None);
            }
        }

        pool
    }

    /// Mark a package name for loading with the given constraint.
    fn mark_package_name_for_loading(&mut self, name: &str, constraint: Box<dyn ConstraintInterface>) {
        let name_lower = name.to_lowercase();

        // Skip platform packages
        if is_platform_package(&name_lower) {
            return;
        }

        // Root require already loaded the maximum range
        if self.max_extended_reqs.contains(&name_lower) {
            return;
        }

        // Not yet loaded, set the constraint to be loaded
        if !self.loaded_packages.contains_key(&name_lower) {
            if let Some(existing) = self.packages_to_load.get(&name_lower) {
                // Already marked for loading - check if we need to extend the constraint
                if self.is_subset_of(&*constraint, &**existing) {
                    return;
                }
                // Extend the constraint by creating an OR
                let new_constraint = self.merge_constraints(&**existing, &*constraint);
                self.packages_to_load.insert(name_lower, new_constraint);
            } else {
                self.packages_to_load.insert(name_lower, constraint);
            }
            return;
        }

        // Already loaded - check if we need to reload with extended constraint
        let loaded_constraint = self.loaded_packages.get(&name_lower).unwrap();
        if self.is_subset_of(&*constraint, &**loaded_constraint) {
            return;
        }

        // Need to reload with extended constraint
        let new_constraint = self.merge_constraints(&**loaded_constraint, &*constraint);
        self.packages_to_load.insert(name_lower.clone(), new_constraint);
        self.loaded_packages.remove(&name_lower);
    }

    /// Load all packages marked for loading from repositories.
    fn load_packages_marked_for_loading(&mut self, repositories: &[&dyn Repository]) {
        // Move packages_to_load to loaded_packages
        let packages_to_load: Vec<_> = self.packages_to_load.drain().collect();

        for (name, constraint) in &packages_to_load {
            self.loaded_packages.insert(name.clone(), constraint.clone_box());
        }

        // Load packages from each repository
        for repo in repositories {
            for (name, constraint) in &packages_to_load {
                // Get packages matching the name
                if let Some(versions) = repo.packages_by_name(name) {
                    for pkg in versions {
                        // Check if version matches constraint
                        let version_constraint = self.parse_version_as_constraint(&pkg.version);
                        if let Some(vc) = version_constraint {
                            if constraint.matches(&vc) {
                                self.load_package(pkg.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    /// Load a package and mark its dependencies for loading.
    fn load_package(&mut self, package: Arc<Package>) {
        let key = (package.name.to_lowercase(), package.version.clone());

        // Skip if already seen
        if self.seen_packages.contains(&key) {
            return;
        }
        self.seen_packages.insert(key);

        // Add to loaded packages
        self.loaded_package_data.push(package.clone());

        // Mark dependencies for loading
        for (dep_name, constraint_str) in &package.require {
            let dep_name_lower = dep_name.to_lowercase();

            // Skip platform packages
            if is_platform_package(&dep_name_lower) {
                continue;
            }

            if let Ok(constraint) = self.version_parser.parse_constraints(constraint_str) {
                self.mark_package_name_for_loading(dep_name, constraint);
            }
        }
    }

    /// Parse a version string as a constraint for matching.
    fn parse_version_as_constraint(&self, version: &str) -> Option<Constraint> {
        // Create an equality constraint for the version
        Constraint::new(phpx_semver::Operator::Equal, version.to_string()).ok()
    }

    /// Check if constraint a is a subset of constraint b.
    fn is_subset_of(&self, a: &dyn ConstraintInterface, b: &dyn ConstraintInterface) -> bool {
        // Simple heuristic: if the string representations are equal, it's a subset
        // A more accurate check would use interval arithmetic
        a.to_string() == b.to_string()
    }

    /// Merge two constraints with OR semantics.
    fn merge_constraints(&self, a: &dyn ConstraintInterface, b: &dyn ConstraintInterface) -> Box<dyn ConstraintInterface> {
        // Create a disjunctive (OR) multi-constraint
        // For simplicity, we just return the broader "*" constraint if either is "*"
        let a_str = a.to_string();
        let b_str = b.to_string();

        if a_str == "*" || b_str == "*" {
            if let Ok(c) = self.version_parser.parse_constraints("*") {
                return c;
            }
        }

        // Try to parse as a combined constraint
        let combined = format!("{} || {}", a_str, b_str);
        if let Ok(c) = self.version_parser.parse_constraints(&combined) {
            return c;
        }

        // Fallback to the more permissive one (just use "*")
        self.version_parser.parse_constraints("*").unwrap()
    }
}

impl Default for PoolBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_builder_new() {
        let builder = PoolBuilder::new();
        assert!(builder.packages_to_load.is_empty());
        assert!(builder.loaded_packages.is_empty());
    }
}
