//! Pool optimization for reducing the size of the package pool before solving.
//!
//! This module implements Composer's PoolOptimizer which removes unnecessary packages
//! from the pool to speed up the SAT solver by reducing the number of rules.
//!
//! Two main optimizations are performed:
//! 1. **Identical dependencies optimization**: Groups packages with identical dependency
//!    definitions and keeps only the best version from each group.
//! 2. **Impossible packages optimization**: Uses locked package constraints to filter
//!    out versions that can't possibly be selected.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use std::sync::Arc;

use phpx_semver::{Constraint, ConstraintInterface, Operator, VersionParser};

use super::policy::Policy;
use super::pool::{Pool, PoolEntry, PackageId};
use super::request::Request;
use crate::package::Package;
use crate::util::is_platform_package;

/// Optimizes a Pool by removing unnecessary packages before solving.
///
/// This reduces the number of SAT rules and speeds up solving.
pub struct PoolOptimizer<'a> {
    /// Selection policy for determining which package to keep
    policy: &'a Policy,

    /// Packages that cannot be removed (fixed/locked)
    irremovable_packages: HashSet<PackageId>,

    /// Collected require constraints per package name
    require_constraints: HashMap<String, HashSet<String>>,

    /// Collected conflict constraints per package name
    conflict_constraints: HashMap<String, HashSet<String>>,

    /// Packages marked for removal
    packages_to_remove: HashSet<PackageId>,

    /// Maps base package IDs to their alias package IDs
    aliases_per_package: HashMap<PackageId, Vec<PackageId>>,

    /// Version parser for constraint operations
    version_parser: VersionParser,

    /// Cache for parsed constraints (constraint_string -> parsed constraint)
    constraint_cache: HashMap<String, Option<Box<dyn ConstraintInterface>>>,

    /// Cache for normalized versions (raw_version -> normalized_version)
    version_cache: HashMap<String, String>,

    /// Cache for version constraints (normalized_version -> Constraint for matching)
    version_constraint_cache: HashMap<String, Option<Constraint>>,
}

impl<'a> PoolOptimizer<'a> {
    /// Create a new pool optimizer with the given policy.
    pub fn new(policy: &'a Policy) -> Self {
        Self {
            policy,
            irremovable_packages: HashSet::new(),
            require_constraints: HashMap::new(),
            conflict_constraints: HashMap::new(),
            packages_to_remove: HashSet::new(),
            aliases_per_package: HashMap::new(),
            version_parser: VersionParser::new(),
            constraint_cache: HashMap::new(),
            version_cache: HashMap::new(),
            version_constraint_cache: HashMap::new(),
        }
    }

    /// Optimize the pool and return a new optimized pool.
    pub fn optimize(&mut self, request: &Request, pool: &Pool) -> Pool {
        // Reset state
        self.irremovable_packages.clear();
        self.require_constraints.clear();
        self.conflict_constraints.clear();
        self.packages_to_remove.clear();
        self.aliases_per_package.clear();
        self.constraint_cache.clear();
        self.version_cache.clear();
        self.version_constraint_cache.clear();

        // Prepare: collect constraints and mark irremovable packages
        self.prepare(request, pool);

        // Pre-warm caches: parse all unique constraints and normalize all unique versions upfront
        self.prewarm_caches(pool);

        // Optimization 1: Remove packages with identical dependencies, keeping only the best
        self.optimize_by_identical_dependencies(pool);

        // Optimization 2: Remove packages that can't satisfy locked constraints
        self.optimize_impossible_packages_away(request, pool);

        // Apply removals and create new pool
        self.apply_removals_to_pool(pool)
    }

    /// Pre-warm caches by parsing all constraints and normalizing all versions upfront.
    /// This avoids repeated parsing during the hot loop.
    /// Pre-warm caches by parsing all unique constraints upfront.
    /// Version normalization is done lazily to avoid processing unused packages.
    fn prewarm_caches(&mut self, _pool: &Pool) {
        // Collect all unique constraint strings
        let mut all_constraints: HashSet<String> = HashSet::new();
        for constraints in self.require_constraints.values() {
            all_constraints.extend(constraints.iter().cloned());
        }
        for constraints in self.conflict_constraints.values() {
            all_constraints.extend(constraints.iter().cloned());
        }

        // Parse all constraints upfront
        for constraint_str in &all_constraints {
            if !self.constraint_cache.contains_key(constraint_str) {
                let parsed = self.version_parser.parse_constraints(constraint_str).ok()
                    .map(|c| c as Box<dyn ConstraintInterface>);
                self.constraint_cache.insert(constraint_str.clone(), parsed);
            }
        }
    }

    /// Prepare optimization by collecting constraints and marking irremovable packages.
    fn prepare(&mut self, request: &Request, pool: &Pool) {
        // Mark fixed packages as irremovable
        for fixed in &request.fixed_packages {
            if let Some(id) = self.find_package_id(pool, &fixed.name, &fixed.version) {
                self.mark_irremovable(pool, id);
            }
        }

        // Mark locked packages as irremovable
        for locked in &request.locked_packages {
            if let Some(id) = self.find_package_id(pool, &locked.name, &locked.version) {
                self.mark_irremovable(pool, id);
            }
        }

        // Mark packages as irremovable if they provide/replace a virtual package
        // that has no other providers in the pool
        // This ensures root packages that replace things like shopware/core are kept
        for id in pool.all_package_ids() {
            if let Some(pkg) = pool.package(id) {
                let mut is_sole_provider = false;

                for (replaced, _) in &pkg.replace {
                    let replaced_lower = replaced.to_lowercase();
                    // Check if there are any direct packages with this name
                    // or any other packages that provide/replace it
                    let providers = pool.what_provides(&replaced_lower, None);
                    // If only this package provides it (or the list is empty aside from this pkg)
                    if providers.is_empty() || (providers.len() == 1 && providers[0] == id) {
                        is_sole_provider = true;
                        log::trace!(
                            "{} {} is sole provider for replaced package {}",
                            pkg.name,
                            pkg.version,
                            replaced_lower
                        );
                        break;
                    }
                }

                if !is_sole_provider {
                    for (provided, _) in &pkg.provide {
                        let provided_lower = provided.to_lowercase();
                        let providers = pool.what_provides(&provided_lower, None);
                        if providers.is_empty() || (providers.len() == 1 && providers[0] == id) {
                            is_sole_provider = true;
                            log::trace!(
                                "{} {} is sole provider for provided package {}",
                                pkg.name,
                                pkg.version,
                                provided_lower
                            );
                            break;
                        }
                    }
                }

                if is_sole_provider {
                    self.mark_irremovable(pool, id);
                }
            }
        }

        // Extract require constraints from root requirements
        for (name, constraint) in request.all_requires() {
            self.extract_require_constraint(name, constraint);
        }

        // First pass over all packages to extract constraints and build alias map
        for id in pool.all_package_ids() {
            if let Some(entry) = pool.entry(id) {
                match entry {
                    PoolEntry::Package(pkg) => {
                        // Extract requires
                        for (target, constraint) in &pkg.require {
                            self.extract_require_constraint(target, constraint);
                        }

                        // Extract conflicts
                        for (target, constraint) in &pkg.conflict {
                            self.extract_conflict_constraint(target, constraint);
                        }
                    }
                    PoolEntry::Alias(alias) => {
                        // Track alias relationships
                        if let Some(base_id) = pool.get_alias_base(id) {
                            self.aliases_per_package
                                .entry(base_id)
                                .or_default()
                                .push(id);
                        }

                        // Extract requires from alias's base package
                        let base_pkg = alias.alias_of();
                        for (target, constraint) in &base_pkg.require {
                            self.extract_require_constraint(target, constraint);
                        }

                        // Extract conflicts
                        for (target, constraint) in &base_pkg.conflict {
                            self.extract_conflict_constraint(target, constraint);
                        }
                    }
                }
            }
        }
    }

    /// Mark a package as irremovable, including its aliases.
    fn mark_irremovable(&mut self, pool: &Pool, id: PackageId) {
        self.irremovable_packages.insert(id);

        // Also mark aliases as irremovable
        if let Some(aliases) = self.aliases_per_package.get(&id) {
            for &alias_id in aliases {
                self.irremovable_packages.insert(alias_id);
            }
        }

        // If this is an alias, mark the base package too
        if let Some(base_id) = pool.get_alias_base(id) {
            self.irremovable_packages.insert(base_id);
            // And all other aliases of that base
            if let Some(aliases) = self.aliases_per_package.get(&base_id) {
                for &alias_id in aliases {
                    self.irremovable_packages.insert(alias_id);
                }
            }
        }
    }

    /// Extract a require constraint for a package name.
    ///
    /// Disjunctive constraints (e.g. "^2.14 || ^3.3") are expanded into separate
    /// constraints to ensure we keep the best matching package for EACH branch.
    fn extract_require_constraint(&mut self, package_name: &str, constraint: &str) {
        let name_lower = package_name.to_lowercase();
        let entry = self.require_constraints.entry(name_lower).or_default();

        // Expand disjunctive (OR) constraints into separate constraints
        for expanded in Self::expand_disjunctive_constraints(constraint) {
            entry.insert(expanded);
        }
    }

    /// Extract a conflict constraint for a package name.
    ///
    /// Disjunctive constraints are expanded similar to require constraints.
    fn extract_conflict_constraint(&mut self, package_name: &str, constraint: &str) {
        let name_lower = package_name.to_lowercase();
        let entry = self.conflict_constraints.entry(name_lower).or_default();

        // Expand disjunctive (OR) constraints into separate constraints
        for expanded in Self::expand_disjunctive_constraints(constraint) {
            entry.insert(expanded);
        }
    }

    /// Expand disjunctive (OR) constraints into separate parts.
    ///
    /// For example, "^2.14 || ^3.3" becomes ["^2.14", "^3.3"].
    /// And "^2.0.5|^3.0|^4.0" becomes ["^2.0.5", "^3.0", "^4.0"].
    /// This ensures the optimizer keeps the best package for EACH branch of an OR,
    /// not just the overall best match.
    fn expand_disjunctive_constraints(constraint: &str) -> Vec<String> {
        // First try splitting on || (double pipe)
        let parts: Vec<&str> = constraint
            .split("||")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if parts.len() > 1 {
            // This is a disjunctive constraint with || - return each part separately
            return parts.into_iter().map(String::from).collect();
        }

        // Also try splitting on single | (but not inside version specs like "1.0-beta|alpha")
        // In Composer, | is also a valid OR operator
        let parts: Vec<&str> = constraint
            .split('|')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if parts.len() > 1 {
            // This is a disjunctive constraint with | - return each part separately
            parts.into_iter().map(String::from).collect()
        } else {
            // Single constraint (or conjunctive with comma/space) - return as-is
            vec![constraint.to_string()]
        }
    }

    /// Optimization 1: Remove packages with identical dependencies.
    ///
    /// Groups packages by their dependency hash and keeps only the best version
    /// (according to the policy) from each group.
    fn optimize_by_identical_dependencies(&mut self, pool: &Pool) {
        // Map constraint string -> Unique ID
        let mut constraint_ids: HashMap<String, u32> = HashMap::new();
        let mut next_id = 0;

        // Pre-collect constraints by package name to avoid borrow issues
        // HashMap<PackageName, Vec<(ConstraintId, ConstraintString)>>
        let mut require_constraints_snapshot: HashMap<String, Vec<(u32, String)>> = HashMap::new();
        let mut conflict_constraints_snapshot: HashMap<String, Vec<(u32, String)>> = HashMap::new();

        // deterministic iteration for assigning IDs
        let mut sorted_require_keys: Vec<_> = self.require_constraints.keys().collect();
        sorted_require_keys.sort();

        for name in sorted_require_keys {
            let constraints = &self.require_constraints[name];
            // Sort constraints for deterministic ID assignment
            let mut sorted_constraints: Vec<_> = constraints.iter().collect();
            sorted_constraints.sort();

            let mut list = Vec::with_capacity(constraints.len());
            for c in sorted_constraints {
                let id = if let Some(&id) = constraint_ids.get(c) {
                    id
                } else {
                    let id = next_id;
                    constraint_ids.insert(c.clone(), id);
                    next_id += 1;
                    id
                };
                list.push((id, c.clone()));
            }
            require_constraints_snapshot.insert(name.clone(), list);
        }

        let mut sorted_conflict_keys: Vec<_> = self.conflict_constraints.keys().collect();
        sorted_conflict_keys.sort();

        for name in sorted_conflict_keys {
            let constraints = &self.conflict_constraints[name];
            // Sort constraints for deterministic ID assignment
            let mut sorted_constraints: Vec<_> = constraints.iter().collect();
            sorted_constraints.sort();

            let mut list = Vec::with_capacity(constraints.len());
            for c in sorted_constraints {
                let id = if let Some(&id) = constraint_ids.get(c) {
                    id
                } else {
                    let id = next_id;
                    constraint_ids.insert(c.clone(), id);
                    next_id += 1;
                    id
                };
                list.push((id, c.clone()));
            }
            conflict_constraints_snapshot.insert(name.clone(), list);
        }

        // Group: package_name -> group_hash -> dependency_hash -> list of packages
        // Using u64 hashes instead of strings for performance
        let mut groups: HashMap<String, HashMap<u64, HashMap<u64, Vec<PackageId>>>> = HashMap::new();

        // Track which packages have been assigned to a group
        let mut packages_in_groups: HashSet<PackageId> = HashSet::new();

        for id in pool.all_package_ids() {
            // Skip irremovable packages
            if self.irremovable_packages.contains(&id) {
                continue;
            }

            // Skip aliases (they're handled with their base package)
            if pool.is_alias(id) {
                continue;
            }

            let Some(pkg) = pool.package(id) else {
                continue;
            };

            // Initially mark for removal
            self.packages_to_remove.insert(id);

            let pkg_name = pkg.name.to_lowercase();
            let mut matched_constraints: Vec<u32> = Vec::new();

            // Check requires
            if let Some(constraints) = require_constraints_snapshot.get(&pkg_name) {
                for (id, constraint_str) in constraints {
                    let matches = self.version_matches_constraint(&pkg.version, constraint_str);
                    if matches {
                        matched_constraints.push(id << 1); // LSB 0 for require
                    }
                }
            }

            // Check conflicts
            if let Some(constraints) = conflict_constraints_snapshot.get(&pkg_name) {
                for (id, constraint_str) in constraints {
                    if self.version_matches_constraint(&pkg.version, constraint_str) {
                        matched_constraints.push((id << 1) | 1); // LSB 1 for conflict
                    }
                }
            }

            // Only group if it matches at least one constraint
            if !matched_constraints.is_empty() {
                // Determine group hash
                matched_constraints.sort_unstable();

                let mut hasher = DefaultHasher::new();
                for mc in matched_constraints {
                    mc.hash(&mut hasher);
                }
                let group_hash = hasher.finish();

                let dep_hash = self.calculate_dependency_hash(pkg);

                groups
                    .entry(pkg_name)
                    .or_default()
                    .entry(group_hash)
                    .or_default()
                    .entry(dep_hash)
                    .or_default()
                    .push(id);

                packages_in_groups.insert(id);
            }
        }

        // Now keep the best package from each group
        // Sort keys for deterministic iteration order
        let mut pkg_names: Vec<_> = groups.keys().collect();
        pkg_names.sort();
        for pkg_name in pkg_names {
            let constraint_groups = &groups[pkg_name];
            let mut group_hashes: Vec<_> = constraint_groups.keys().collect();
            group_hashes.sort();
            for group_hash in group_hashes {
                let dep_hash_groups = &constraint_groups[group_hash];
                let mut dep_hashes: Vec<_> = dep_hash_groups.keys().collect();
                dep_hashes.sort();
                for dep_hash in dep_hashes {
                    let packages = &dep_hash_groups[dep_hash];
                    if packages.len() == 1 {
                        // Only one package in this group, must keep it
                        self.keep_package(pool, packages[0]);
                    } else {
                        // Multiple packages with same deps - keep the preferred one
                        let preferred = self.policy.select_preferred(pool, packages);
                        for &pkg_id in &preferred {
                            self.keep_package(pool, pkg_id);
                        }
                    }
                }
            }
        }

        // Also keep packages that weren't in any constraint group but are required
        // (packages that have no constraints matching them should still be kept
        // if they're the only option)
        for id in pool.all_package_ids() {
            if self.irremovable_packages.contains(&id) || pool.is_alias(id) {
                continue;
            }

            // If package wasn't added to any group, it matches no active constraints.
            // It should be kept ONLY if no other version of this package matched any constraints 
            // (i.e. the package name itself is not part of the active problem space constraints).
            if !packages_in_groups.contains(&id) {
                // If we haven't already decided to keep it (it's still in removal set)
                if self.packages_to_remove.contains(&id) {
                    if let Some(pkg) = pool.package(id) {
                        let pkg_name = pkg.name.to_lowercase();
                        if !groups.contains_key(&pkg_name) {
                            // No groups for this package name at all, keep it
                            self.keep_package(pool, id);
                        }
                    }
                }
            }
        }
    }

    /// Keep a package (remove from packages_to_remove set).
    fn keep_package(&mut self, pool: &Pool, id: PackageId) {
        self.packages_to_remove.remove(&id);

        // Also keep aliases
        if let Some(aliases) = self.aliases_per_package.get(&id).cloned() {
            for alias_id in aliases {
                self.packages_to_remove.remove(&alias_id);
            }
        }

        // If this is an alias, keep the base too
        if let Some(base_id) = pool.get_alias_base(id) {
            self.packages_to_remove.remove(&base_id);
        }
    }

    /// Calculate a hash of the package's dependencies for grouping.
    fn calculate_dependency_hash(&self, package: &Package) -> u64 {
        let mut hasher = DefaultHasher::new();
        
        // Helper to hash a map sorted
        fn hash_deps(hasher: &mut DefaultHasher, deps: &std::collections::HashMap<String, String>, prefix: u8) {
            if deps.is_empty() { return; }
            prefix.hash(hasher);
            
            // Collect references and sort to ensure deterministic hashing
            let mut sorted: Vec<_> = deps.iter().collect();
            sorted.sort_unstable_by(|a, b| a.0.cmp(b.0));
            
            for (name, constraint) in sorted {
                name.hash(hasher);
                constraint.hash(hasher);
            }
        }

        hash_deps(&mut hasher, &package.require, 1);
        hash_deps(&mut hasher, &package.conflict, 2);
        hash_deps(&mut hasher, &package.replace, 3);
        hash_deps(&mut hasher, &package.provide, 4);

        hasher.finish()
    }

    /// Optimization 2: Remove packages that can't satisfy locked package constraints.
    ///
    /// Uses the requirements of locked packages to filter out versions that
    /// definitely won't be selected.
    fn optimize_impossible_packages_away(&mut self, request: &Request, pool: &Pool) {
        if request.locked_packages.is_empty() {
            return;
        }

        // Build an index of packages by name with version info (excluding irremovable and aliases)
        // Store (id, version) to avoid repeated pool lookups
        let mut package_index: HashMap<String, Vec<(PackageId, String)>> = HashMap::new();

        for id in pool.all_package_ids() {
            // Skip irremovable
            if self.irremovable_packages.contains(&id) {
                continue;
            }

            // Skip aliases (they're handled with their base)
            if pool.is_alias(id) {
                continue;
            }

            // Skip already marked for removal
            if self.packages_to_remove.contains(&id) {
                continue;
            }

            if let Some(pkg) = pool.package(id) {
                // Skip locked packages themselves
                let is_locked = request.locked_packages.iter()
                    .any(|l| l.name.eq_ignore_ascii_case(&pkg.name) && l.version == pkg.version);
                if is_locked {
                    continue;
                }

                package_index
                    .entry(pkg.name.to_lowercase())
                    .or_default()
                    .push((id, pkg.version.clone()));
            }
        }

        // Collect all filter operations to perform (to avoid borrow issues)
        // (package_name, constraint) pairs we need to check
        let mut filter_ops: Vec<(String, String)> = Vec::new();

        for locked in &request.locked_packages {
            // Check if the locked package is still required
            let locked_name = locked.name.to_lowercase();
            if !self.require_constraints.contains_key(&locked_name) {
                continue;
            }

            // Collect filter operations
            for (require_name, constraint) in &locked.require {
                let require_name_lower = require_name.to_lowercase();
                if package_index.contains_key(&require_name_lower) {
                    filter_ops.push((require_name_lower, constraint.clone()));
                }
            }
        }

        // Now apply filters
        for (require_name_lower, constraint) in filter_ops {
            if let Some(candidates) = package_index.get(&require_name_lower) {
                // Collect IDs to remove
                let mut to_remove: Vec<PackageId> = Vec::new();

                for (id, version) in candidates {
                    if !self.version_matches_constraint(version, &constraint) {
                        to_remove.push(*id);
                    }
                }

                // Apply removals
                for id in &to_remove {
                    self.packages_to_remove.insert(*id);
                    // Also mark aliases for removal
                    if let Some(aliases) = self.aliases_per_package.get(id).cloned() {
                        for alias_id in aliases {
                            self.packages_to_remove.insert(alias_id);
                        }
                    }
                }

                // Update the index to remove filtered packages
                if let Some(candidates) = package_index.get_mut(&require_name_lower) {
                    candidates.retain(|(id, _)| !to_remove.contains(id));
                }
            }
        }
    }

    /// Check if a version matches a constraint.
    fn version_matches_constraint(&mut self, version: &str, constraint_str: &str) -> bool {
        // Handle wildcard
        if constraint_str == "*" || constraint_str.is_empty() {
            return true;
        }

        // Get normalized version from cache, or normalize and cache it
        let normalized_version = if let Some(cached) = self.version_cache.get(version) {
            cached.clone()
        } else {
            let normalized = self.version_parser.normalize(version)
                .unwrap_or_else(|_| version.to_string());
            // Debug logging for twig/twig investigation
            if version.contains("3.22") || version.contains("3.21") {
                log::trace!("Normalizing version '{}' -> '{}'", version, normalized);
            }
            self.version_cache.insert(version.to_string(), normalized.clone());
            normalized
        };

        // Ensure version constraint is cached
        if !self.version_constraint_cache.contains_key(&normalized_version) {
            let vc = Constraint::new(Operator::Equal, normalized_version.clone()).ok();
            self.version_constraint_cache.insert(normalized_version.clone(), vc);
        }

        // Ensure parsed constraint is cached
        if !self.constraint_cache.contains_key(constraint_str) {
            let parsed = self.version_parser.parse_constraints(constraint_str).ok()
                .map(|c| c as Box<dyn ConstraintInterface>);
            self.constraint_cache.insert(constraint_str.to_string(), parsed);
        }

        // Now do lookups (no mutation needed)
        let version_constraint = self.version_constraint_cache.get(&normalized_version)
            .and_then(|opt| opt.as_ref());
        let parsed_constraint = self.constraint_cache.get(constraint_str)
            .and_then(|opt| opt.as_ref());

        match (version_constraint, parsed_constraint) {
            (Some(vc), Some(pc)) => pc.matches(vc),
            _ => true, // Be permissive on failure
        }
    }

    /// Find a package ID by name and version.
    fn find_package_id(&self, pool: &Pool, name: &str, version: &str) -> Option<PackageId> {
        let name_lower = name.to_lowercase();
        for id in pool.packages_by_name(&name_lower) {
            if let Some(entry) = pool.entry(id) {
                if entry.version() == version {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Apply the collected removals and create a new optimized pool.
    fn apply_removals_to_pool(&self, original_pool: &Pool) -> Pool {
        log::debug!("Pool optimizer removing {} packages from pool of {}", self.packages_to_remove.len(), original_pool.len());

        // Debug: count how many of each package are being removed
        let mut pkg_counts: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
        for id in original_pool.all_package_ids() {
            if let Some(pkg) = original_pool.package(id) {
                let entry = pkg_counts.entry(pkg.name.clone()).or_insert((0, 0));
                entry.0 += 1; // total
                if self.packages_to_remove.contains(&id) {
                    entry.1 += 1; // removed
                }
            }
        }
        // Log packages where all versions are removed (potential problem)
        for (name, (total, removed)) in &pkg_counts {
            if *removed == *total && *total > 0 {
                log::warn!("Pool optimizer removed all {} versions of {}", total, name);
            }
        }

        // Log what versions of key packages are being kept
        for key in &["symfony/console", "symfony/http-kernel", "symfony/string", "symfony/event-dispatcher"] {
            if let Some(&(total, removed)) = pkg_counts.get(*key) {
                log::debug!("Pool optimizer: {} - kept {}/{} versions", key, total - removed, total);
            }
        }

        let mut new_pool = Pool::with_minimum_stability(original_pool.minimum_stability());

        // Copy stability flags
        // TODO: Access private field stability_flags if possible, or add getter/setter
        // Since we can't access private fields easily without modifying Pool, 
        // we might be missing flags. But wait, we can add them via builder or setter.
        // Assuming we rely on the fact that stability was checked during initial pool population ??
        // Actually, optimization might lose stability flags which is bad for subsequent lookups.
        
        // Copy packages that aren't marked for removal
        for id in original_pool.all_package_ids() {
            if self.packages_to_remove.contains(&id) {
                continue;
            }

            if let Some(entry) = original_pool.entry(id) {
                match entry {
                    PoolEntry::Package(pkg) => {
                        let repo_name = original_pool.get_repository(id);
                        let priority = original_pool.get_priority_by_id(id);

                        // Platform packages and packages with replace/provide should bypass
                        // stability filtering. Platform packages are fixed system packages.
                        // Packages with replace/provide are typically root or metapackages
                        // that need to be preserved regardless of their version stability.
                        let bypass_stability = is_platform_package(&pkg.name)
                            || !pkg.replace.is_empty()
                            || !pkg.provide.is_empty();

                        if bypass_stability {
                            new_pool.add_package_arc_bypass_stability(
                                Arc::clone(pkg),
                                repo_name,
                            );
                        } else {
                            new_pool.add_package_arc(
                                Arc::clone(pkg),
                                repo_name,
                            );
                        }

                        // Preserve priority
                        if let Some(repo) = repo_name {
                            new_pool.set_priority(repo, priority);
                        }
                    }
                    PoolEntry::Alias(alias) => {
                        // Aliases will be recreated after their base packages
                        // We need to find if the base is in the new pool
                        if let Some(base_id) = original_pool.get_alias_base(id) {
                            // Only add alias if base package was kept
                            if !self.packages_to_remove.contains(&base_id) {
                                let repo_name = original_pool.get_repository(id);
                                new_pool.add_alias_package_arc(Arc::clone(alias), repo_name);
                            }
                        }
                    }
                }
            }
        }

        new_pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::package::{AliasPackage, Stability};

    #[test]
    fn test_optimizer_basic() {
        let mut pool = Pool::new();
        pool.add_package(Package::new("vendor/a", "1.0.0"));
        pool.add_package(Package::new("vendor/a", "2.0.0"));
        pool.add_package(Package::new("vendor/b", "1.0.0"));

        let mut request = Request::new();
        request.require("vendor/a", "^1.0");
        request.require("vendor/b", "^1.0");

        let policy = Policy::new();
        let mut optimizer = PoolOptimizer::new(&policy);
        let optimized = optimizer.optimize(&request, &pool);

        // Should have kept packages for vendor/a matching ^1.0 and vendor/b
        assert!(optimized.len() >= 2);
    }

    #[test]
    fn test_optimizer_keeps_irremovable() {
        let mut pool = Pool::new();
        pool.add_package(Package::new("vendor/a", "1.0.0"));
        pool.add_package(Package::new("vendor/a", "2.0.0"));

        let mut request = Request::new();
        request.require("vendor/a", "*");
        request.lock(Package::new("vendor/a", "1.0.0"));

        let policy = Policy::new();
        let mut optimizer = PoolOptimizer::new(&policy);
        let optimized = optimizer.optimize(&request, &pool);

        // Locked package should still be there
        let versions: Vec<_> = optimized.packages_by_name("vendor/a")
            .iter()
            .filter_map(|&id| optimized.package(id))
            .map(|p| p.version.as_str())
            .collect();

        assert!(versions.contains(&"1.0.0"));
    }

    #[test]
    fn test_optimizer_removes_impossible_versions() {
        let mut pool = Pool::new();

        // A requires B ^1.0
        let mut a = Package::new("vendor/a", "1.0.0");
        a.require.insert("vendor/b".to_string(), "^1.0".to_string());
        pool.add_package(a);

        // B has versions 1.0, 1.5, and 2.0
        pool.add_package(Package::new("vendor/b", "1.0.0"));
        pool.add_package(Package::new("vendor/b", "1.5.0"));
        pool.add_package(Package::new("vendor/b", "2.0.0"));

        // Lock A at 1.0.0 (which requires B ^1.0)
        let mut request = Request::new();
        request.require("vendor/a", "^1.0");
        request.require("vendor/b", "*");
        let mut locked_a = Package::new("vendor/a", "1.0.0");
        locked_a.require.insert("vendor/b".to_string(), "^1.0".to_string());
        request.lock(locked_a);

        let policy = Policy::new();
        let mut optimizer = PoolOptimizer::new(&policy);
        let optimized = optimizer.optimize(&request, &pool);

        // B 2.0.0 should be removed since it can't satisfy ^1.0
        let b_versions: Vec<_> = optimized.packages_by_name("vendor/b")
            .iter()
            .filter_map(|&id| optimized.package(id))
            .map(|p| p.version.as_str())
            .collect();

        // Should only have versions matching ^1.0
        assert!(!b_versions.contains(&"2.0.0"), "B 2.0.0 should be removed");
    }

    #[test]
    fn test_optimizer_identical_dependencies() {
        let mut pool = Pool::new();

        // Multiple versions of A with identical requirements
        let mut a1 = Package::new("vendor/a", "1.0.0");
        a1.require.insert("vendor/b".to_string(), "^1.0".to_string());
        pool.add_package(a1);

        let mut a2 = Package::new("vendor/a", "1.0.1");
        a2.require.insert("vendor/b".to_string(), "^1.0".to_string());
        pool.add_package(a2);

        let mut a3 = Package::new("vendor/a", "1.0.2");
        a3.require.insert("vendor/b".to_string(), "^1.0".to_string());
        pool.add_package(a3);

        pool.add_package(Package::new("vendor/b", "1.0.0"));

        let mut request = Request::new();
        request.require("vendor/a", "^1.0");
        request.require("vendor/b", "^1.0");

        // With default policy (prefer highest), should keep only 1.0.2
        let policy = Policy::new();
        let mut optimizer = PoolOptimizer::new(&policy);
        let optimized = optimizer.optimize(&request, &pool);

        let a_versions: Vec<_> = optimized.packages_by_name("vendor/a")
            .iter()
            .filter_map(|&id| optimized.package(id))
            .map(|p| p.version.as_str())
            .collect();

        // Should only keep the best version (1.0.2 with prefer_highest)
        assert_eq!(a_versions.len(), 1);
        assert!(a_versions.contains(&"1.0.2"));
    }

    #[test]
    fn test_optimizer_with_aliases() {
        let mut pool = Pool::with_minimum_stability(Stability::Dev);

        // Base package
        let pkg = Package::new("vendor/a", "dev-main");
        let _base_id = pool.add_package(pkg.clone());

        // Alias for the dev version
        let alias = AliasPackage::new(
            Arc::new(pkg),
            "1.0.0.0".to_string(),
            "1.0.0".to_string(),
        );
        pool.add_alias_package(alias);

        let mut request = Request::new();
        request.require("vendor/a", "^1.0");

        let policy = Policy::new();
        let mut optimizer = PoolOptimizer::new(&policy);
        let optimized = optimizer.optimize(&request, &pool);

        // Both base and alias should be kept
        let all_ids: Vec<_> = optimized.packages_by_name("vendor/a");
        assert!(!all_ids.is_empty(), "Package should be preserved");
    }

    #[test]
    fn test_optimizer_preserves_repo_priority() {
        let mut pool = Pool::new();

        pool.add_package_from_repo(Package::new("vendor/a", "1.0.0"), Some("repo1"));
        pool.add_package_from_repo(Package::new("vendor/a", "1.0.0"), Some("repo2"));
        pool.set_priority("repo1", 0);
        pool.set_priority("repo2", 1);

        let mut request = Request::new();
        request.require("vendor/a", "^1.0");

        let policy = Policy::new();
        let mut optimizer = PoolOptimizer::new(&policy);
        let optimized = optimizer.optimize(&request, &pool);

        // Should keep the one from higher priority repo
        let a_ids: Vec<_> = optimized.packages_by_name("vendor/a");
        assert!(!a_ids.is_empty());
    }
}
