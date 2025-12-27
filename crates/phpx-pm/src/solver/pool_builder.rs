//! Demand-driven pool builder for efficient package loading.
//!
//! This module implements a pool builder that only loads packages that are
//! reachable from root requirements, similar to PHP Composer's PoolBuilder.
//! This dramatically reduces the pool size compared to loading all packages upfront.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::pool::Pool;
use super::request::Request;
use crate::package::{AliasPackage, Package, parse_branch_aliases};
use crate::repository::Repository;
use crate::util::is_platform_package;

/// Batch size for loading packages (matches PHP Composer)
const LOAD_BATCH_SIZE: usize = 50;

/// Builds a pool by demand-driven loading of packages.
///
/// Instead of loading all packages upfront, this builder:
/// 1. Starts with root requirements
/// 2. Loads packages matching those requirements in batches
/// 3. Recursively loads dependencies of loaded packages
/// 4. Only includes packages that are actually reachable
pub struct PoolBuilder {
    /// Packages marked for loading (name -> constraint string)
    packages_to_load: HashMap<String, String>,

    /// Packages already loaded (name -> constraint that was loaded)
    loaded_packages: HashMap<String, String>,

    /// The packages that have been loaded into the pool
    loaded_package_data: Vec<Arc<Package>>,

    /// Aliases to add to the pool
    aliases: Vec<AliasPackage>,

    /// Root requirements (should not be extended by transitive deps)
    max_extended_reqs: HashSet<String>,

    /// Track which packages we've seen to avoid duplicates
    seen_packages: HashSet<(String, String)>,

    /// Names that have been definitively found in a repository
    /// (to skip looking in lower-priority repos)
    names_found: HashSet<String>,
}

impl PoolBuilder {
    /// Create a new pool builder.
    pub fn new() -> Self {
        Self {
            packages_to_load: HashMap::new(),
            loaded_packages: HashMap::new(),
            loaded_package_data: Vec::new(),
            aliases: Vec::new(),
            max_extended_reqs: HashSet::new(),
            seen_packages: HashSet::new(),
            names_found: HashSet::new(),
        }
    }

    /// Build a pool from repositories using demand-driven loading.
    pub async fn build_pool(
        &mut self,
        repositories: &[Arc<dyn Repository>],
        request: &Request,
    ) -> Pool {
        let start = std::time::Instant::now();

        // Reset state
        self.packages_to_load.clear();
        self.loaded_packages.clear();
        self.loaded_package_data.clear();
        self.aliases.clear();
        self.max_extended_reqs.clear();
        self.seen_packages.clear();
        self.names_found.clear();

        // Step 1: Mark fixed/locked packages as loaded
        for fixed in &request.fixed_packages {
            let name_lower = fixed.name.to_lowercase();
            let constraint = format!("={}", fixed.version);
            self.loaded_packages.insert(name_lower, constraint);
        }

        for locked in &request.locked_packages {
            let name_lower = locked.name.to_lowercase();
            let constraint = format!("={}", locked.version);
            self.loaded_packages.insert(name_lower.clone(), constraint);

            // Also mark replaced packages as loaded (replace = conflict with all versions)
            for (replaced, _) in &locked.replace {
                let replaced_lower = replaced.to_lowercase();
                if !self.loaded_packages.contains_key(&replaced_lower) {
                    self.loaded_packages.insert(replaced_lower, "*".to_string());
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

            self.packages_to_load
                .insert(name_lower.clone(), constraint_str.to_string());
            self.max_extended_reqs.insert(name_lower);
        }

        // Step 3: Load packages in waves until nothing left to load
        let mut iteration = 0;
        while !self.packages_to_load.is_empty() {
            iteration += 1;
            log::debug!(
                "PoolBuilder iteration {}: {} packages to load",
                iteration,
                self.packages_to_load.len()
            );
            self.load_packages_marked_for_loading(repositories).await;
        }

        log::info!(
            "PoolBuilder loaded {} packages in {} iterations ({:?})",
            self.loaded_package_data.len(),
            iteration,
            start.elapsed()
        );

        // Step 4: Build the pool from loaded packages
        let mut pool = Pool::new();

        for package in &self.loaded_package_data {
            pool.add_package_arc(package.clone(), None);
        }

        for alias in &self.aliases {
            pool.add_alias_package(alias.clone());
        }

        // Add fixed packages to the pool
        for fixed in &request.fixed_packages {
            let existing = self.loaded_package_data.iter().find(|p| {
                p.name.eq_ignore_ascii_case(&fixed.name) && p.version == fixed.version
            });

            if existing.is_none() {
                let pkg = Package {
                    name: fixed.name.clone(),
                    version: fixed.version.clone(),
                    ..Default::default()
                };
                pool.add_package(pkg);
            }
        }

        // Add locked packages to the pool
        for locked in &request.locked_packages {
            let existing = self.loaded_package_data.iter().find(|p| {
                p.name.eq_ignore_ascii_case(&locked.name) && p.version == locked.version
            });

            if existing.is_none() {
                pool.add_package_arc(locked.clone(), None);
            }
        }

        pool
    }

    /// Mark a package name for loading with the given constraint.
    fn mark_package_name_for_loading(&mut self, name: &str, constraint: &str) {
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
                if self.is_subset_of(constraint, existing) {
                    return;
                }
                // Extend the constraint by creating an OR
                let new_constraint = self.merge_constraints(existing, constraint);
                self.packages_to_load.insert(name_lower, new_constraint);
            } else {
                self.packages_to_load
                    .insert(name_lower, constraint.to_string());
            }
            return;
        }

        // Already loaded - check if we need to reload with extended constraint
        let loaded_constraint = self.loaded_packages.get(&name_lower).unwrap().clone();
        if self.is_subset_of(constraint, &loaded_constraint) {
            return;
        }

        // Need to reload with extended constraint
        let new_constraint = self.merge_constraints(&loaded_constraint, constraint);
        self.packages_to_load.insert(name_lower.clone(), new_constraint);
        self.loaded_packages.remove(&name_lower);
    }

    /// Load all packages marked for loading from repositories using batch loading.
    async fn load_packages_marked_for_loading(&mut self, repositories: &[Arc<dyn Repository>]) {
        // Move packages_to_load to loaded_packages
        let packages_to_load: Vec<_> = self.packages_to_load.drain().collect();

        for (name, constraint) in &packages_to_load {
            self.loaded_packages.insert(name.clone(), constraint.clone());
        }

        // Split into batches
        let batches: Vec<_> = packages_to_load
            .chunks(LOAD_BATCH_SIZE)
            .map(|chunk| chunk.to_vec())
            .collect();

        // Process each batch across all repositories
        for batch in batches {
            // Filter out packages that were already found in higher-priority repos
            let batch_to_load: Vec<(String, Option<String>)> = batch
                .iter()
                .filter(|(name, _)| !self.names_found.contains(name))
                .map(|(name, constraint)| (name.clone(), Some(constraint.clone())))
                .collect();

            if batch_to_load.is_empty() {
                continue;
            }

            // Load from each repository in priority order
            for repo in repositories {
                let result = repo.load_packages_batch(&batch_to_load).await;

                // Track which names were found
                for name in result.names_found {
                    self.names_found.insert(name.to_lowercase());
                }

                // Process loaded packages
                for pkg in result.packages {
                    self.load_package(pkg);
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

        // Parse and add branch aliases
        let branch_aliases = parse_branch_aliases(package.extra.as_ref());
        for (source_version, (alias_normalized, alias_pretty)) in branch_aliases {
            // Only add alias if the package version matches the source version
            if package.version == source_version || package.pretty_version.as_deref() == Some(&source_version) {
                let alias = AliasPackage::new(
                    package.clone(),
                    alias_normalized,
                    alias_pretty,
                );
                self.aliases.push(alias);
            }
        }

        // Mark dependencies for loading
        for (dep_name, constraint_str) in &package.require {
            let dep_name_lower = dep_name.to_lowercase();

            // Skip platform packages
            if is_platform_package(&dep_name_lower) {
                continue;
            }

            self.mark_package_name_for_loading(dep_name, constraint_str);
        }
    }

    /// Check if constraint a is a subset of constraint b.
    fn is_subset_of(&self, a: &str, b: &str) -> bool {
        // Simple heuristic: if the string representations are equal, it's a subset
        // A "*" constraint is never a subset of anything except itself
        if a == b {
            return true;
        }
        if b == "*" {
            return true; // Everything is a subset of *
        }
        false
    }

    /// Merge two constraints with OR semantics.
    fn merge_constraints(&self, a: &str, b: &str) -> String {
        // If either is "*", return "*"
        if a == "*" || b == "*" {
            return "*".to_string();
        }

        // Try to combine with OR
        format!("{} || {}", a, b)
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

    #[test]
    fn test_is_subset_of() {
        let builder = PoolBuilder::new();
        assert!(builder.is_subset_of("^1.0", "^1.0"));
        assert!(builder.is_subset_of("^1.0", "*"));
        assert!(!builder.is_subset_of("*", "^1.0"));
    }

    #[test]
    fn test_merge_constraints() {
        let builder = PoolBuilder::new();
        assert_eq!(builder.merge_constraints("^1.0", "^2.0"), "^1.0 || ^2.0");
        assert_eq!(builder.merge_constraints("*", "^1.0"), "*");
        assert_eq!(builder.merge_constraints("^1.0", "*"), "*");
    }
}
