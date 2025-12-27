use std::collections::HashSet;

use super::pool::{Pool, PackageId, PoolEntry};
use super::request::Request;
use super::rule::{Rule, RuleType};
use super::rule_set::RuleSet;
use crate::util::is_platform_package;

/// Generates SAT rules from a dependency graph.
///
/// This converts the dependency relationships into SAT clauses:
/// - Root requirements: at least one version must be installed
/// - Package requirements: if A is installed, then B|C|D must be installed
/// - Conflicts: A and B cannot both be installed
/// - Same-name: only one version of a package can be installed
/// - Provider conflicts: packages providing/replacing the same name conflict
/// - Alias rules: if an alias is installed, its base package must be installed
pub struct RuleGenerator<'a> {
    pool: &'a Pool,
    rules: RuleSet,
    /// Packages we've already processed (by ID)
    added_packages: HashSet<PackageId>,
    /// Packages we've processed, grouped by name (for same-name conflict rules)
    /// This matches Composer's addedPackagesByNames
    added_packages_by_name: std::collections::HashMap<String, Vec<PackageId>>,
    /// Track which names have packages providing/replacing them (name -> package ids)
    providers_by_name: std::collections::HashMap<String, Vec<PackageId>>,
    /// Package names that are explicitly required by the user (root requirements)
    /// Providers/replacers of these packages can be auto-selected
    root_required_names: HashSet<String>,
}

impl<'a> RuleGenerator<'a> {
    /// Create a new rule generator
    pub fn new(pool: &'a Pool) -> Self {
        Self {
            pool,
            rules: RuleSet::new(),
            added_packages: HashSet::new(),
            added_packages_by_name: std::collections::HashMap::new(),
            providers_by_name: std::collections::HashMap::new(),
            root_required_names: HashSet::new(),
        }
    }

    /// Generate all rules for a request
    pub fn generate(mut self, request: &Request) -> RuleSet {
        let start = std::time::Instant::now();

        // Collect all root required package names first
        // This is used to determine if providers/replacers can be auto-selected
        for (name, _) in request.all_requires() {
            self.root_required_names.insert(name.to_lowercase());
        }

        // Also add all packages that provide/replace root requirements
        // so that if user requires "D" which replaces "C", when A requires C,
        // D can be used to satisfy C
        for name in self.root_required_names.clone() {
            let providers = self.pool.what_provides(&name, None);
            for id in providers {
                if let Some(pkg) = self.pool.package(id) {
                    // Add all names this package provides/replaces to root_required_names
                    for provided_name in pkg.get_names(true) {
                        self.root_required_names.insert(provided_name.to_lowercase());
                    }
                }
            }
        }

        // Add names from fixed packages' replace/provide to root_required_names
        // Fixed packages (like the root package) are always installed, so their
        // replaced/provided names should be available to satisfy other dependencies
        for fixed in &request.fixed_packages {
            let ids = self.pool.packages_by_name(&fixed.name);
            for id in ids {
                if let Some(pkg) = self.pool.package(id) {
                    if pkg.version == fixed.version {
                        for provided_name in pkg.get_names(true) {
                            self.root_required_names.insert(provided_name.to_lowercase());
                        }
                        break;
                    }
                }
            }
        }

        // Add fixed package rules first
        self.add_fixed_rules(request);
        log::debug!("After fixed rules: {} rules", self.rules.len());

        // Add root requirement rules
        self.add_root_require_rules(request);
        log::debug!("After root require rules: {} rules, {} packages", self.rules.len(), self.added_packages.len());

        // Add same-name conflict rules (only one version of each package can be installed)
        // This is done AFTER processing all packages, so we only create rules for
        // packages that are actually reachable from root requirements
        self.add_same_name_conflict_rules();
        log::debug!("After same-name conflict rules: {} rules", self.rules.len());

        // Add conflict rules for all processed packages
        self.add_conflict_rules();
        log::debug!("After conflict rules: {} rules", self.rules.len());

        // Add provider conflict rules (packages providing/replacing same name)
        self.add_provider_conflict_rules();
        log::debug!("After provider conflict rules: {} rules", self.rules.len());

        log::info!("Rule generation stats: {} packages processed, {} unique package names, {} provider names tracked in {:?}",
            self.added_packages.len(),
            self.added_packages_by_name.len(),
            self.providers_by_name.len(),
            start.elapsed()
        );
        log::debug!("Rules by type: {:?}", self.rules.stats());

        self.rules
    }

    /// Add rules for fixed packages (must be installed)
    fn add_fixed_rules(&mut self, request: &Request) {
        for package in &request.fixed_packages {
            // Find the package in the pool
            let ids = self.pool.packages_by_name(&package.name);
            for id in ids {
                if let Some(pkg) = self.pool.package(id) {
                    if pkg.version == package.version {
                        let rule = Rule::fixed(id)
                            .with_source(id)
                            .with_target(&package.name);
                        self.rules.add(rule);
                        self.add_package_rules(id);
                        break;
                    }
                }
            }
        }
    }

    /// Add rules for root requirements
    fn add_root_require_rules(&mut self, request: &Request) {
        for (name, constraint) in request.all_requires() {
            // For root requirements, include all packages (direct + providers/replacers)
            // since the user is explicitly requiring this package
            let providers = self.pool.what_provides(name, Some(constraint));

            if providers.is_empty() {
                // No packages satisfy this requirement
                // Add an empty rule that will cause a conflict
                log::warn!(
                    "No packages satisfy root requirement {} {}. Pool has {} versions of {}:",
                    name,
                    constraint,
                    self.pool.packages_by_name(name).len(),
                    name
                );
                // Log all versions from pool for debugging (limit to 20)
                for id in self.pool.packages_by_name(name).iter().take(20) {
                    if let Some(pkg) = self.pool.package(*id) {
                        log::warn!("  - {} {} (stability: {:?})", pkg.name, pkg.version, pkg.stability());
                    }
                }
                // Log what versions are available in the pool
                log::warn!("Available versions in pool for {}:", name);
                let all_versions: Vec<_> = self.pool.packages_by_name(name).iter()
                    .filter_map(|id| self.pool.package(*id))
                    .map(|p| p.version.clone())
                    .collect();
                log::warn!("  All versions: {:?}", all_versions);
                let rule = Rule::new(vec![], RuleType::RootRequire)
                    .with_target(name)
                    .with_constraint(constraint);
                self.rules.add(rule);
                continue;
            }

            // At least one of the providers must be installed
            let rule = Rule::root_require(providers.clone())
                .with_target(name)
                .with_constraint(constraint);
            self.rules.add(rule);

            // Add dependency rules for each provider
            for id in providers {
                self.add_package_rules(id);
            }
        }
    }

    /// Add all rules for a package (requirements, conflicts, same-name)
    fn add_package_rules(&mut self, package_id: PackageId) {
        if self.added_packages.contains(&package_id) {
            return;
        }
        self.added_packages.insert(package_id);

        // Check if this is an alias - if so, add alias-specific rules
        if let Some(entry) = self.pool.entry(package_id) {
            if let PoolEntry::Alias(alias) = entry {
                // If alias is installed, base package must be installed
                if let Some(base_id) = self.pool.get_alias_base(package_id) {
                    // Rule: alias -> base (if alias is installed, base must be installed)
                    let rule = Rule::requires(package_id, vec![base_id])
                        .with_source(package_id)
                        .with_target(alias.name());
                    self.rules.add(rule);

                    // Also process the base package's rules
                    self.add_package_rules(base_id);
                }

                // Process alias dependencies (may differ from base due to self.version replacement)
                for (dep_name, constraint) in alias.require() {
                    if dep_name.starts_with("lib-") {
                        continue;
                    }

                    let providers = self.pool.what_provides(dep_name, Some(constraint));
                    if providers.is_empty() {
                        let rule = Rule::new(vec![-package_id], RuleType::PackageRequires)
                            .with_source(package_id)
                            .with_target(dep_name)
                            .with_constraint(constraint);
                        self.rules.add(rule);
                    } else {
                        let rule = Rule::requires(package_id, providers.clone())
                            .with_source(package_id)
                            .with_target(dep_name)
                            .with_constraint(constraint);
                        self.rules.add(rule);

                        for id in providers {
                            if !self.pool.is_alias(id) {
                                if let Some(pkg) = self.pool.package(id) {
                                    if !is_platform_package(&pkg.name) {
                                        self.add_package_rules(id);
                                    }
                                }
                            }
                        }
                    }
                }

                return;
            }
        }

        let Some(package) = self.pool.package(package_id) else {
            return;
        };

        let package = package.clone();

        // Track this package by all its names (name + replaces, NOT provides)
        // for same-name conflict rules. This matches Composer's addedPackagesByNames pattern.
        // PHP: foreach ($package->getNames(false) as $name) { $this->addedPackagesByNames[$name][] = $package; }
        for name in package.get_names(false) {
            self.added_packages_by_name
                .entry(name)
                .or_default()
                .push(package_id);
        }

        // Track all names this package provides/replaces for later conflict detection
        for name in package.get_names(true) {
            self.providers_by_name
                .entry(name)
                .or_default()
                .push(package_id);
        }

        // Add requirement rules
        for (dep_name, constraint) in &package.require {
            // Skip lib-* packages (library constraints like lib-libxml)
            // These are rarely used and hard to detect
            if dep_name.starts_with("lib-") {
                continue;
            }

            // Composer behavior: providers/replacers are only auto-selected if:
            // 1. There's also a direct package available, OR
            // 2. The dependency name is explicitly required by the user (root requirement)
            //    or provided/replaced by a root-required package
            let direct_providers = self.pool.what_provides_direct_only(dep_name, Some(constraint));
            let has_direct = !direct_providers.is_empty();
            let is_root_required = self.root_required_names.contains(&dep_name.to_lowercase());

            // Get all providers (direct + provide/replace)
            let all_providers = self.pool.what_provides(dep_name, Some(constraint));

            // Include providers/replacers if there's a direct package OR this is a root requirement
            let providers = if has_direct || is_root_required {
                // Include all providers
                all_providers
            } else {
                // Only include direct matches (which is empty), causing the requirement to fail
                // unless the provider/replacer is explicitly required elsewhere
                direct_providers
            };

            if providers.is_empty() {
                // Dependency cannot be satisfied - if this package is installed, conflict
                let rule = Rule::new(vec![-package_id], RuleType::PackageRequires)
                    .with_source(package_id)
                    .with_target(dep_name)
                    .with_constraint(constraint);
                self.rules.add(rule);
                continue;
            }

            // If package_id is installed, one of providers must be installed
            let rule = Rule::requires(package_id, providers.clone())
                .with_source(package_id)
                .with_target(dep_name)
                .with_constraint(constraint);
            self.rules.add(rule);

            // Recursively process dependencies (skip platform packages)
            for id in providers {
                if let Some(pkg) = self.pool.package(id) {
                    // Platform packages (php, ext-*) don't have dependencies to process
                    if !is_platform_package(&pkg.name) {
                        self.add_package_rules(id);
                    }
                }
            }
        }


        // Note: explicit conflict rules are added later in add_conflict_rules()
        // after all packages have been processed. This matches PHP Composer's approach.
    }

    /// Add same-name conflict rules for all processed packages.
    /// Called once at the end of generate() - only processes packages that were
    /// actually added during rule generation, not all packages in the pool.
    /// This matches Composer's behavior in RuleSetGenerator.php lines 242-246.
    fn add_same_name_conflict_rules(&mut self) {
        for (name, package_ids) in &self.added_packages_by_name {
            if package_ids.len() <= 1 {
                continue;
            }

            // Filter out alias-base pairs (they must coexist)
            let mut non_alias_versions: Vec<PackageId> = Vec::new();
            for &id in package_ids {
                // Skip if this is an alias of another version in the list
                if let Some(base_id) = self.pool.get_alias_base(id) {
                    if package_ids.contains(&base_id) {
                        continue; // Skip alias, keep only base
                    }
                }
                non_alias_versions.push(id);
            }

            if non_alias_versions.len() <= 1 {
                continue;
            }

            // Use a single multi-conflict rule instead of O(n²) pairwise conflicts
            // This is much more efficient for packages with many versions
            let rule = Rule::multi_conflict(non_alias_versions)
                .with_target(name);
            self.rules.add(rule);
        }
    }

    /// Add conflict rules for packages that conflict with each other.
    /// This matches PHP Composer's addConflictRules() method.
    ///
    /// NOTE: This can generate a large number of rules when the pool contains
    /// many versions of packages that other packages conflict with. PHP Composer
    /// avoids this by using demand-driven package loading (only loading packages
    /// reachable from root requirements). Our current approach loads all packages
    /// upfront, which can lead to rule explosion in monorepos or when many packages
    /// declare conflicts with common dependencies.
    fn add_conflict_rules(&mut self) {
        let mut conflict_count = 0usize;
        let mut skipped_not_added = 0usize;

        for &package_id in &self.added_packages {
            let Some(package) = self.pool.package(package_id) else {
                continue;
            };
            let package = package.clone();

            // Process explicit conflicts from package's "conflict" field
            for (conflict_name, constraint) in &package.conflict {
                let conflict_name_lower = conflict_name.to_lowercase();

                // Skip if the conflict target is not in our processed packages
                // PHP: if (!isset($this->addedPackagesByNames[$link->getTarget()])) { continue; }
                if !self.added_packages_by_name.contains_key(&conflict_name_lower) {
                    continue;
                }

                // Get matching packages from the pool, but only consider ones we've actually processed
                let conflicting = self.pool.what_provides(conflict_name, Some(constraint));
                for conflict_id in conflicting {
                    if conflict_id != package_id {
                        // Only create conflict rules for packages we've actually added
                        if !self.added_packages.contains(&conflict_id) {
                            skipped_not_added += 1;
                            continue;
                        }

                        // Skip alias conflicts unless the name matches exactly
                        // PHP: if (!$conflict instanceof AliasPackage || $conflict->getName() === $link->getTarget())
                        if self.pool.is_alias(conflict_id) {
                            if let Some(entry) = self.pool.entry(conflict_id) {
                                if let Some(alias) = entry.as_alias() {
                                    if alias.name().to_lowercase() != conflict_name_lower {
                                        continue;
                                    }
                                }
                            }
                        }

                        conflict_count += 1;
                        let rule = Rule::conflict(vec![package_id, conflict_id])
                            .with_source(package_id)
                            .with_target(conflict_name);
                        self.rules.add(rule);
                    }
                }
            }
        }

        log::debug!("add_conflict_rules: {} conflict rules added, {} skipped (not in added_packages)",
            conflict_count, skipped_not_added);
    }

    /// Add conflict rules for packages that REPLACE the same name.
    ///
    /// Note: Packages that merely `provide` a virtual package do NOT conflict
    /// with each other. Multiple packages can provide the same virtual package
    /// and be installed together (e.g., both symfony/console and symfony/http-kernel
    /// can provide psr/log-implementation).
    ///
    /// Only packages that `replace` the same name conflict, because replace
    /// means "this package replaces another package entirely".
    fn add_provider_conflict_rules(&mut self) {
        // Build a map of name -> packages that REPLACE it (not just provide)
        let mut replacers_by_name: std::collections::HashMap<String, Vec<PackageId>> =
            std::collections::HashMap::new();

        for &package_id in &self.added_packages {
            let Some(package) = self.pool.package(package_id) else {
                continue;
            };

            // Only track replaces, not provides
            for (replaced_name, _) in &package.replace {
                let name = replaced_name.to_lowercase();
                replacers_by_name
                    .entry(name)
                    .or_default()
                    .push(package_id);
            }
        }

        // Add multi-conflict rules for packages that replace the same name
        for (name, replacer_ids) in replacers_by_name {
            if replacer_ids.len() <= 1 {
                continue;
            }

            // Skip if we've already added same-name conflict rules for this name
            // (this happens when there are actual packages with this name)
            if self.added_packages_by_name.contains_key(&name) {
                continue;
            }

            // Use multi-conflict rule: at most one replacer can be installed
            let rule = Rule::multi_conflict(replacer_ids)
                .with_target(&name);
            self.rules.add(rule);
        }
    }
}

/// Builder for creating rules with additional context
#[allow(dead_code)]
pub struct RuleBuilder {
    rule: Rule,
}

#[allow(dead_code)]
impl RuleBuilder {
    pub fn new(rule: Rule) -> Self {
        Self { rule }
    }

    pub fn source(mut self, package_id: PackageId) -> Self {
        self.rule = self.rule.with_source(package_id);
        self
    }

    pub fn target(mut self, name: impl Into<String>) -> Self {
        self.rule = self.rule.with_target(name);
        self
    }

    pub fn constraint(mut self, constraint: impl Into<String>) -> Self {
        self.rule = self.rule.with_constraint(constraint);
        self
    }

    pub fn build(self) -> Rule {
        self.rule
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Package;

    fn create_test_pool() -> Pool {
        let mut pool = Pool::new();

        // Add package A with two versions
        let mut a1 = Package::new("vendor/a", "1.0.0");
        a1.require.insert("vendor/b".to_string(), "^1.0".to_string());
        pool.add_package(a1);

        let mut a2 = Package::new("vendor/a", "2.0.0");
        a2.require.insert("vendor/b".to_string(), "^2.0".to_string());
        pool.add_package(a2);

        // Add package B with two versions
        pool.add_package(Package::new("vendor/b", "1.0.0"));
        pool.add_package(Package::new("vendor/b", "2.0.0"));

        // Add package C that conflicts with B
        let mut c = Package::new("vendor/c", "1.0.0");
        c.conflict.insert("vendor/b".to_string(), "*".to_string());
        pool.add_package(c);

        pool
    }

    #[test]
    fn test_rule_generator_root_require() {
        let pool = create_test_pool();
        let mut request = Request::new();
        request.require("vendor/a", "^1.0");

        let generator = RuleGenerator::new(&pool);
        let rules = generator.generate(&request);

        // Should have root requirement rule
        let root_rules: Vec<_> = rules.rules_of_type(RuleType::RootRequire).collect();
        assert!(!root_rules.is_empty());
    }

    #[test]
    fn test_rule_generator_same_name() {
        let pool = create_test_pool();
        let mut request = Request::new();
        request.require("vendor/a", "*");

        let generator = RuleGenerator::new(&pool);
        let rules = generator.generate(&request);

        // Should have multi-conflict rules for vendor/a versions (only one version allowed)
        let multi_conflict_rules: Vec<_> = rules.rules_of_type(RuleType::MultiConflict).collect();
        assert!(!multi_conflict_rules.is_empty());
    }

    #[test]
    fn test_rule_generator_package_requires() {
        let pool = create_test_pool();
        let mut request = Request::new();
        request.require("vendor/a", "*");

        let generator = RuleGenerator::new(&pool);
        let rules = generator.generate(&request);

        // Should have package requirement rules
        let require_rules: Vec<_> = rules.rules_of_type(RuleType::PackageRequires).collect();
        assert!(!require_rules.is_empty());
    }

    #[test]
    fn test_rule_generator_fixed() {
        let pool = create_test_pool();
        let mut request = Request::new();
        request.fix(Package::new("vendor/b", "1.0.0"));
        request.require("vendor/a", "*");

        let generator = RuleGenerator::new(&pool);
        let rules = generator.generate(&request);

        // Should have fixed package rule
        let fixed_rules: Vec<_> = rules.rules_of_type(RuleType::Fixed).collect();
        assert!(!fixed_rules.is_empty());
    }

    #[test]
    fn test_rule_generator_stats() {
        let pool = create_test_pool();
        let mut request = Request::new();
        request.require("vendor/a", "*");

        let generator = RuleGenerator::new(&pool);
        let rules = generator.generate(&request);

        let stats = rules.stats();
        println!("Rules generated: {:?}", stats);
        assert!(stats.total > 0);
    }

    #[test]
    fn test_rule_generator_processes_phpunit_dependencies() {
        // Regression test: packages like phpunit/* must not be skipped as platform packages
        let mut pool = Pool::new();

        // Add phpunit/phpunit which requires phpunit/php-code-coverage
        let mut phpunit = Package::new("phpunit/phpunit", "10.0.0");
        phpunit.require.insert("phpunit/php-code-coverage".to_string(), "^10.0".to_string());
        pool.add_package(phpunit);

        // Add phpunit/php-code-coverage which requires theseer/tokenizer
        let mut coverage = Package::new("phpunit/php-code-coverage", "10.0.0");
        coverage.require.insert("theseer/tokenizer".to_string(), "^1.2".to_string());
        pool.add_package(coverage);

        // Add theseer/tokenizer
        pool.add_package(Package::new("theseer/tokenizer", "1.2.0"));

        let mut request = Request::new();
        request.require("phpunit/phpunit", "*");

        let generator = RuleGenerator::new(&pool);
        let rules = generator.generate(&request);

        let require_rules: Vec<_> = rules.rules_of_type(RuleType::PackageRequires).collect();
        assert!(require_rules.len() >= 2);
    }

    #[test]
    fn test_rule_generator_skips_actual_platform_packages() {
        let mut pool = Pool::new();

        let mut package = Package::new("vendor/package", "1.0.0");
        package.require.insert("php".to_string(), "^8.0".to_string());
        package.require.insert("ext-json".to_string(), "*".to_string());
        pool.add_package(package);

        pool.add_platform_package(Package::new("php", "8.2.0"));
        pool.add_platform_package(Package::new("ext-json", "8.2.0"));

        let mut request = Request::new();
        request.require("vendor/package", "*");

        let generator = RuleGenerator::new(&pool);
        let rules = generator.generate(&request);

        let require_rules: Vec<_> = rules.rules_of_type(RuleType::PackageRequires).collect();
        assert!(!require_rules.is_empty());
    }
}
