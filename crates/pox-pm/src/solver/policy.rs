use std::collections::{BTreeMap, HashMap};
use super::pool::{Pool, PackageId};

/// Policy for selecting between candidate packages.
///
/// When multiple packages can satisfy a requirement, the policy
/// determines which one to try first.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// Prefer stable versions over dev
    pub prefer_stable: bool,
    /// Prefer lowest versions (for testing)
    pub prefer_lowest: bool,
    /// Prefer dev versions over prerelease (alpha/beta/RC) when prefer_lowest is true
    /// This matches Composer's COMPOSER_PREFER_DEV_OVER_PRERELEASE env var behavior
    pub prefer_dev_over_prerelease: bool,
    /// Preferred versions for specific packages (package name -> normalized version)
    /// When a preferred version is available, it will be selected over newer versions
    pub preferred_versions: HashMap<String, String>,
}

impl Policy {
    /// Create a new policy with default settings
    pub fn new() -> Self {
        Self {
            prefer_stable: true,
            prefer_lowest: false,
            prefer_dev_over_prerelease: false,
            preferred_versions: HashMap::new(),
        }
    }

    /// Set preference for stable versions
    pub fn prefer_stable(mut self, prefer: bool) -> Self {
        self.prefer_stable = prefer;
        self
    }

    /// Set preference for lowest versions
    pub fn prefer_lowest(mut self, prefer: bool) -> Self {
        self.prefer_lowest = prefer;
        self
    }

    /// Set preference for dev versions over prerelease versions
    /// Only applies when prefer_lowest is true
    pub fn prefer_dev_over_prerelease(mut self, prefer: bool) -> Self {
        self.prefer_dev_over_prerelease = prefer;
        self
    }

    /// Set preferred versions for specific packages
    pub fn preferred_versions(mut self, versions: HashMap<String, String>) -> Self {
        self.preferred_versions = versions;
        self
    }

    /// Add a preferred version for a specific package
    pub fn with_preferred_version(mut self, package: &str, version: &str) -> Self {
        self.preferred_versions.insert(package.to_lowercase(), version.to_string());
        self
    }

    /// Select the preferred package from candidates.
    ///
    /// Returns the candidates sorted by preference (best first).
    /// This implements Composer's package selection logic:
    /// 1. Prefer aliases over non-aliases (for same package name)
    /// 2. Prefer original packages over replacers
    /// 3. Prefer same vendor as the required package
    /// 4. Prefer by version (highest/lowest based on policy)
    /// 5. Fall back to package ID (pool insertion order)
    pub fn select_preferred(&self, pool: &Pool, candidates: &[PackageId]) -> Vec<PackageId> {
        self.select_preferred_for_requirement(pool, candidates, None)
    }

    /// Select preferred packages considering the required package name.
    /// This allows preferring packages from the same vendor.
    pub fn select_preferred_for_requirement(
        &self,
        pool: &Pool,
        candidates: &[PackageId],
        required_package: Option<&str>,
    ) -> Vec<PackageId> {
        if candidates.is_empty() {
            return Vec::new();
        }

        // Group candidates by package name (use BTreeMap for deterministic ordering)
        let mut by_name: BTreeMap<String, Vec<PackageId>> = BTreeMap::new();
        for &id in candidates {
            if let Some(pkg) = pool.package(id) {
                by_name.entry(pkg.name.to_lowercase()).or_default().push(id);
            }
        }

        for group in by_name.values_mut() {
            group.sort_by(|&a, &b| {
                self.compare_by_priority(pool, a, b, required_package, true)
            });
        }

        // Flatten and sort across all groups
        let mut result: Vec<PackageId> = by_name.into_values().flatten().collect();

        // Final sort respecting replacers across packages
        result.sort_by(|&a, &b| {
            self.compare_by_priority(pool, a, b, required_package, false)
        });

        result
    }

    /// Compare two packages by priority (Composer's compareByPriority logic).
    fn compare_by_priority(
        &self,
        pool: &Pool,
        a: PackageId,
        b: PackageId,
        required_package: Option<&str>,
        ignore_replace: bool,
    ) -> std::cmp::Ordering {
        let pkg_a = pool.package(a);
        let pkg_b = pool.package(b);

        match (pkg_a, pkg_b) {
            (Some(pa), Some(pb)) => {
                // Prefer root package aliases over other aliases
                let a_is_root_alias = pool.is_root_package_alias(a);
                let b_is_root_alias = pool.is_root_package_alias(b);
                if a_is_root_alias && !b_is_root_alias {
                    return std::cmp::Ordering::Less; // prefer a (root alias)
                }
                if !a_is_root_alias && b_is_root_alias {
                    return std::cmp::Ordering::Greater; // prefer b (root alias)
                }

                // Prefer aliases over non-aliases for same package name
                if pa.name.to_lowercase() == pb.name.to_lowercase() {
                    let a_is_alias = pool.is_alias(a);
                    let b_is_alias = pool.is_alias(b);
                    if a_is_alias && !b_is_alias {
                        return std::cmp::Ordering::Less; // prefer a (alias)
                    }
                    if !a_is_alias && b_is_alias {
                        return std::cmp::Ordering::Greater; // prefer b (alias)
                    }
                }

                if !ignore_replace {
                    // Prefer original packages over replacers
                    // If a replaces b's name, prefer b (the original)
                    if self.replaces(pa, &pb.name) {
                        return std::cmp::Ordering::Greater; // prefer b
                    }
                    if self.replaces(pb, &pa.name) {
                        return std::cmp::Ordering::Less; // prefer a
                    }

                    // Prefer same vendor as required package
                    if let Some(req_pkg) = required_package {
                        if let Some(req_vendor) = req_pkg.split('/').next() {
                            let a_same_vendor = pa.name.starts_with(&format!("{}/", req_vendor));
                            let b_same_vendor = pb.name.starts_with(&format!("{}/", req_vendor));
                            if a_same_vendor && !b_same_vendor {
                                return std::cmp::Ordering::Less; // prefer a
                            }
                            if !a_same_vendor && b_same_vendor {
                                return std::cmp::Ordering::Greater; // prefer b
                            }
                        }
                    }
                }

                // Compare repository priority (lower priority number = higher preference)
                let priority_a = pool.get_priority_by_id(a);
                let priority_b = pool.get_priority_by_id(b);
                if priority_a != priority_b {
                    return priority_a.cmp(&priority_b); // lower priority = preferred
                }

                // Compare stability if prefer_stable is set
                if self.prefer_stable {
                    use crate::package::Stability;
                    let stab_a = pa.stability();
                    let stab_b = pb.stability();

                    if self.prefer_lowest && self.prefer_dev_over_prerelease {
                        let a_is_dev = stab_a == Stability::Dev;
                        let b_is_dev = stab_b == Stability::Dev;
                        let a_is_prerelease = matches!(stab_a, Stability::Alpha | Stability::Beta | Stability::RC);
                        let b_is_prerelease = matches!(stab_b, Stability::Alpha | Stability::Beta | Stability::RC);

                        if a_is_dev && b_is_prerelease {
                            return std::cmp::Ordering::Less;
                        }
                        if b_is_dev && a_is_prerelease {
                            return std::cmp::Ordering::Greater;
                        }
                    }

                    let stability_cmp = stab_a.priority().cmp(&stab_b.priority());
                    if stability_cmp != std::cmp::Ordering::Equal {
                        return stability_cmp;
                    }
                }

                if !self.preferred_versions.is_empty() {
                    let pkg_name = pa.name.to_lowercase();
                    if let Some(preferred) = self.preferred_versions.get(&pkg_name) {
                        let a_is_preferred = self.versions_match(&pa.version, preferred);
                        let b_is_preferred = self.versions_match(&pb.version, preferred);
                        if a_is_preferred && !b_is_preferred {
                            return std::cmp::Ordering::Less;
                        }
                        if !a_is_preferred && b_is_preferred {
                            return std::cmp::Ordering::Greater;
                        }
                    }
                }

                // Compare versions
                let version_cmp = compare_versions(&pa.version, &pb.version);
                let version_result = if self.prefer_lowest {
                    version_cmp
                } else {
                    version_cmp.reverse()
                };

                if version_result != std::cmp::Ordering::Equal {
                    return version_result;
                }

                // Fall back to package ID (pool insertion order)
                a.cmp(&b)
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    }

    /// Check if source package replaces target package name.
    fn replaces(&self, source: &crate::package::Package, target_name: &str) -> bool {
        source.replace.keys().any(|replaced| replaced.eq_ignore_ascii_case(target_name))
    }

    /// Check if two version strings match (normalized comparison).
    /// Composer uses normalized versions like "1.1.0.0" for matching.
    fn versions_match(&self, version: &str, preferred: &str) -> bool {
        // Normalize both versions by extracting numeric parts
        let normalize = |v: &str| -> Vec<u32> {
            v.split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect()
        };

        let v1 = normalize(version);
        let v2 = normalize(preferred);

        // Pad to same length with zeros
        let max_len = v1.len().max(v2.len());
        let v1_padded: Vec<u32> = v1.iter().copied().chain(std::iter::repeat(0)).take(max_len).collect();
        let v2_padded: Vec<u32> = v2.iter().copied().chain(std::iter::repeat(0)).take(max_len).collect();

        v1_padded == v2_padded
    }

    /// Compare versions respecting stability and prefer_lowest settings.
    /// Returns Ordering::Less if a is better than b.
    fn version_compare(&self, a: &crate::package::Package, b: &crate::package::Package) -> std::cmp::Ordering {
        use crate::package::Stability;

        // First compare stability if prefer_stable is set
        if self.prefer_stable {
            let stab_a = a.stability();
            let stab_b = b.stability();

            // Special case: prefer_dev_over_prerelease with prefer_lowest
            // When set, dev versions are preferred over prerelease (alpha/beta/RC)
            if self.prefer_lowest && self.prefer_dev_over_prerelease {
                let a_is_dev = stab_a == Stability::Dev;
                let b_is_dev = stab_b == Stability::Dev;
                let a_is_prerelease = matches!(stab_a, Stability::Alpha | Stability::Beta | Stability::RC);
                let b_is_prerelease = matches!(stab_b, Stability::Alpha | Stability::Beta | Stability::RC);

                // Dev is preferred over prerelease when this flag is set
                if a_is_dev && b_is_prerelease {
                    return std::cmp::Ordering::Less; // a (dev) is better
                }
                if b_is_dev && a_is_prerelease {
                    return std::cmp::Ordering::Greater; // b (dev) is better
                }
            }

            let stab_a_priority = stab_a.priority();
            let stab_b_priority = stab_b.priority();
            if stab_a_priority != stab_b_priority {
                // Lower priority number = more stable = better
                return stab_a_priority.cmp(&stab_b_priority);
            }
        }

        // Then compare versions
        let version_cmp = compare_versions(&a.version, &b.version);
        if self.prefer_lowest {
            version_cmp
        } else {
            version_cmp.reverse()
        }
    }

    /// Select a single best package from candidates
    pub fn select_best(&self, pool: &Pool, candidates: &[PackageId]) -> Option<PackageId> {
        self.select_preferred(pool, candidates).into_iter().next()
    }

    /// Select the best package(s) from candidates for pool optimization.
    /// Unlike select_preferred which returns all candidates sorted,
    /// this returns only the best version(s) for pruning the pool.
    pub fn select_preferred_for_optimization(&self, pool: &Pool, candidates: &[PackageId]) -> Vec<PackageId> {
        if candidates.is_empty() {
            return Vec::new();
        }

        // Group candidates by package name
        let mut by_name: BTreeMap<String, Vec<PackageId>> = BTreeMap::new();
        for &id in candidates {
            if let Some(pkg) = pool.package(id) {
                by_name.entry(pkg.name.to_lowercase()).or_default().push(id);
            }
        }

        // For each group, sort and keep only the best version
        let mut result = Vec::new();
        for (_, group) in by_name.iter_mut() {
            group.sort_by(|&a, &b| {
                self.compare_by_priority(pool, a, b, None, true)
            });

            // Keep only the best version(s) from this group
            if !group.is_empty() {
                // Get best package info
                let best_pkg = pool.package(group[0]);
                let best_priority = pool.get_priority_by_id(group[0]);
                result.push(group[0]);

                // Also include packages with same version and priority
                for &id in &group[1..] {
                    let pkg = pool.package(id);
                    let priority = pool.get_priority_by_id(id);

                    if priority == best_priority {
                        match (pkg, best_pkg) {
                            (Some(p), Some(bp)) if self.version_compare(p, bp) == std::cmp::Ordering::Equal => {
                                result.push(id);
                            }
                            _ => break,
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        result
    }
}

/// Simple version comparison.
/// Returns Ordering::Greater if a > b (a is newer).
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts_a: Vec<u32> = a
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();

    let parts_b: Vec<u32> = b
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();

    let max_len = parts_a.len().max(parts_b.len());

    for i in 0..max_len {
        let pa = parts_a.get(i).copied().unwrap_or(0);
        let pb = parts_b.get(i).copied().unwrap_or(0);

        match pa.cmp(&pb) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }

    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Package;

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("1.0.0", "1.0.0"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("2.0.0", "1.0.0"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "2.0.0"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("1.10.0", "1.9.0"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.0.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_policy_prefer_highest() {
        let mut pool = Pool::new();
        let id1 = pool.add_package(Package::new("vendor/pkg", "1.0.0"));
        let id2 = pool.add_package(Package::new("vendor/pkg", "2.0.0"));
        let id3 = pool.add_package(Package::new("vendor/pkg", "1.5.0"));

        let policy = Policy::new();
        let sorted = policy.select_preferred(&pool, &[1, 2, 3]);

        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0], id2);
        assert_eq!(sorted[1], id3);
        assert_eq!(sorted[2], id1);
    }

    #[test]
    fn test_policy_prefer_lowest() {
        let mut pool = Pool::new();
        let id1 = pool.add_package(Package::new("vendor/pkg", "1.0.0"));
        let id2 = pool.add_package(Package::new("vendor/pkg", "2.0.0"));
        let id3 = pool.add_package(Package::new("vendor/pkg", "1.5.0"));

        let policy = Policy::new().prefer_lowest(true);
        let sorted = policy.select_preferred(&pool, &[1, 2, 3]);

        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0], id1);
        assert_eq!(sorted[1], id3);
        assert_eq!(sorted[2], id2);
    }

    #[test]
    fn test_policy_prefer_stable() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id1 = pool.add_package(Package::new("vendor/pkg", "2.0.0-dev"));
        let id2 = pool.add_package(Package::new("vendor/pkg", "1.0.0"));

        let policy = Policy::new().prefer_stable(true);
        let sorted = policy.select_preferred(&pool, &[1, 2]);

        assert_eq!(sorted.len(), 2);
        assert_eq!(sorted[0], id2);
        assert_eq!(sorted[1], id1);
    }

    #[test]
    fn test_policy_select_best() {
        let mut pool = Pool::new();
        pool.add_package(Package::new("vendor/pkg", "1.0.0"));
        let id2 = pool.add_package(Package::new("vendor/pkg", "2.0.0"));

        let policy = Policy::new();
        let best = policy.select_best(&pool, &[1, 2]);

        assert_eq!(best, Some(id2));
    }

    #[test]
    fn test_policy_prefer_original_over_replacer() {
        let mut pool = Pool::new();

        // Original package
        let id1 = pool.add_package(Package::new("vendor/original", "1.0.0"));

        // Replacer package
        let mut replacer = Package::new("vendor/replacer", "1.0.0");
        replacer.replace.insert("vendor/original".to_string(), "*".to_string());
        let id2 = pool.add_package(replacer);

        let policy = Policy::new();
        let sorted = policy.select_preferred_for_requirement(&pool, &[id1, id2], Some("vendor/original"));

        // Original should be preferred over replacer
        assert_eq!(sorted[0], id1);
    }

    // =========================================================================
    // Tests ported from Composer's DefaultPolicyTest.php
    // =========================================================================

    /// Port of Composer's testSelectSingle
    #[test]
    fn test_select_single() {
        let mut pool = Pool::new();
        let id_a = pool.add_package(Package::new("a", "1.0.0"));

        let policy = Policy::new();
        let selected = policy.select_preferred(&pool, &[id_a]);

        assert_eq!(selected, vec![id_a]);
    }

    /// Port of Composer's testSelectNewest
    #[test]
    fn test_select_newest() {
        let mut pool = Pool::new();
        let id_a1 = pool.add_package(Package::new("a", "1.0.0"));
        let id_a2 = pool.add_package(Package::new("a", "2.0.0"));

        let policy = Policy::new();
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // Should have newest (2.0.0) first
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_a2);
        assert_eq!(selected[1], id_a1);
    }

    /// Port of Composer's testSelectNewestPicksLatest
    /// When prefer_stable is false, picks latest even if unstable
    #[test]
    fn test_select_newest_picks_latest() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id_a1 = pool.add_package(Package::new("a", "1.0.0"));
        let id_a2 = pool.add_package(Package::new("a", "1.0.1-alpha"));

        // With prefer_stable=false, should pick the alpha (newer version) first
        let policy = Policy::new().prefer_stable(false);
        let selected = policy.select_preferred(&pool, &[1, 2]);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_a2);
        assert_eq!(selected[1], id_a1);
    }

    /// Port of Composer's testSelectNewestPicksLatestStableWithPreferStable
    #[test]
    fn test_select_newest_picks_latest_stable_with_prefer_stable() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id_a1 = pool.add_package(Package::new("a", "1.0.0"));
        let id_a2 = pool.add_package(Package::new("a", "1.0.1-alpha"));

        // With prefer_stable=true (default), should have stable 1.0.0 first
        let policy = Policy::new().prefer_stable(true);
        let selected = policy.select_preferred(&pool, &[1, 2]);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_a1);
        assert_eq!(selected[1], id_a2);
    }

    /// Port of Composer's testSelectNewestWithDevPicksNonDev
    #[test]
    fn test_select_newest_with_dev_picks_non_dev() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id_a1 = pool.add_package(Package::new("a", "dev-foo"));
        let id_a2 = pool.add_package(Package::new("a", "1.0.0"));

        let policy = Policy::new();
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // Should have stable 1.0.0 first
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_a2);
        assert_eq!(selected[1], id_a1);
    }

    /// Port of Composer's testSelectLowest
    #[test]
    fn test_select_lowest() {
        let mut pool = Pool::new();
        let id_a1 = pool.add_package(Package::new("a", "1.0.0"));
        let id_a2 = pool.add_package(Package::new("a", "2.0.0"));

        let policy = Policy::new().prefer_lowest(true);
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // Should have lowest (1.0.0) first
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_a1);
        assert_eq!(selected[1], id_a2);
    }

    /// Port of Composer's testSelectLowestPrefersPrereleaseOverDev
    /// With prefer_stable and prefer_lowest, prerelease is preferred over dev
    #[test]
    fn test_select_lowest_prefers_prerelease_over_dev() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id_dev = pool.add_package(Package::new("a", "dev-master"));
        let id_prerelease = pool.add_package(Package::new("a", "1.0.0-alpha1"));

        // prefer_stable=true, prefer_lowest=true
        let policy = Policy::new().prefer_stable(true).prefer_lowest(true);
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // Alpha is more stable than dev, so it should be first
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_prerelease);
        assert_eq!(selected[1], id_dev);
    }

    /// Port of Composer's testSelectLowestWithPreferStableStillPrefersStable
    #[test]
    fn test_select_lowest_with_prefer_stable_still_prefers_stable() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id_stable = pool.add_package(Package::new("a", "1.0.0"));
        let id_dev = pool.add_package(Package::new("a", "dev-master"));

        // prefer_stable=true, prefer_lowest=true
        let policy = Policy::new().prefer_stable(true).prefer_lowest(true);
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // Stable is preferred even with prefer_lowest
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_stable);
        assert_eq!(selected[1], id_dev);
    }

    /// Port of Composer's testRepositoryOrderingAffectsPriority
    #[test]
    fn test_repository_ordering_affects_priority() {
        let mut pool = Pool::new();

        // Repo1 packages (added first = higher priority)
        let id1 = pool.add_package_from_repo(Package::new("a", "1.0.0"), Some("repo1"));
        let id2 = pool.add_package_from_repo(Package::new("a", "1.1.0"), Some("repo1"));
        // Repo2 packages (added second = lower priority)
        let id3 = pool.add_package_from_repo(Package::new("a", "1.1.0"), Some("repo2"));
        let id4 = pool.add_package_from_repo(Package::new("a", "1.2.0"), Some("repo2"));

        pool.set_priority("repo1", 0); // higher priority
        pool.set_priority("repo2", 1); // lower priority

        let policy = Policy::new();
        let selected = policy.select_preferred(&pool, &[1, 2, 3, 4]);

        // Should have 1.1.0 from repo1 (higher priority repo, highest version in that repo) first
        assert_eq!(selected.len(), 4);
        assert_eq!(selected[0], id2); // 1.1.0 from repo1 (best)
        assert_eq!(selected[1], id1); // 1.0.0 from repo1
        // repo2 packages come after since lower priority
        assert!(selected.contains(&id3));
        assert!(selected.contains(&id4));
    }

    /// Port of Composer's testSelectAllProviders
    /// When packages provide a virtual package, all providers should be returned
    #[test]
    fn test_select_all_providers() {
        let mut pool = Pool::new();

        let mut pkg_a = Package::new("a", "1.0.0");
        pkg_a.provide.insert("x".to_string(), "1.0.0".to_string());
        let id_a = pool.add_package(pkg_a);

        let mut pkg_b = Package::new("b", "2.0.0");
        pkg_b.provide.insert("x".to_string(), "1.0.0".to_string());
        let id_b = pool.add_package(pkg_b);

        let policy = Policy::new();
        // When both are providers of the same virtual package, both should be returned
        let selected = policy.select_preferred(&pool, &[id_a, id_b]);

        // Both providers should be in the result (different package names)
        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&id_a));
        assert!(selected.contains(&id_b));
    }

    /// Port of Composer's testPreferNonReplacingFromSameRepo
    #[test]
    fn test_prefer_non_replacing_from_same_repo() {
        let mut pool = Pool::new();

        let pkg_a = Package::new("a", "1.0.0");
        let id_a = pool.add_package(pkg_a);

        let mut pkg_b = Package::new("b", "2.0.0");
        pkg_b.replace.insert("a".to_string(), "1.0.0".to_string());
        let id_b = pool.add_package(pkg_b);

        let policy = Policy::new();
        // When looking for "a", should prefer the original over the replacer
        let selected = policy.select_preferred_for_requirement(&pool, &[id_a, id_b], Some("a"));

        // Both should be returned since they're different packages,
        // but original (A) should come first
        assert_eq!(selected[0], id_a);
    }

    /// Port of Composer's testPreferReplacingPackageFromSameVendor
    #[test]
    fn test_prefer_replacing_package_from_same_vendor() {
        let mut pool = Pool::new();

        let mut pkg_b = Package::new("vendor-b/replacer", "1.0.0");
        pkg_b.replace.insert("vendor-a/package".to_string(), "1.0.0".to_string());
        let id_b = pool.add_package(pkg_b);

        let mut pkg_a = Package::new("vendor-a/replacer", "1.0.0");
        pkg_a.replace.insert("vendor-a/package".to_string(), "1.0.0".to_string());
        let id_a = pool.add_package(pkg_a);

        let policy = Policy::new();
        // When looking for vendor-a/package, should prefer vendor-a/replacer
        let selected = policy.select_preferred_for_requirement(&pool, &[id_b, id_a], Some("vendor-a/package"));

        // vendor-a/replacer should come first (same vendor)
        assert_eq!(selected[0], id_a);
    }

    /// Port of Composer's testSelectLowestWithPreferDevOverPrerelease
    /// Tests with alpha, beta, and RC stabilities
    #[test]
    fn test_select_lowest_with_prefer_dev_over_prerelease_alpha() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id_dev = pool.add_package(Package::new("a", "dev-master"));
        let id_prerelease = pool.add_package(Package::new("a", "1.0.0-alpha1"));

        // prefer_stable=true, prefer_lowest=true, prefer_dev_over_prerelease=true
        let policy = Policy::new()
            .prefer_stable(true)
            .prefer_lowest(true)
            .prefer_dev_over_prerelease(true);
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // With prefer_dev_over_prerelease, dev is preferred over alpha
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_dev);
        assert_eq!(selected[1], id_prerelease);
    }

    #[test]
    fn test_select_lowest_with_prefer_dev_over_prerelease_beta() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id_dev = pool.add_package(Package::new("a", "dev-master"));
        let id_prerelease = pool.add_package(Package::new("a", "1.0.0-beta1"));

        let policy = Policy::new()
            .prefer_stable(true)
            .prefer_lowest(true)
            .prefer_dev_over_prerelease(true);
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // With prefer_dev_over_prerelease, dev is preferred over beta
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_dev);
        assert_eq!(selected[1], id_prerelease);
    }

    #[test]
    fn test_select_lowest_with_prefer_dev_over_prerelease_rc() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);
        let id_dev = pool.add_package(Package::new("a", "dev-master"));
        let id_prerelease = pool.add_package(Package::new("a", "1.0.0-RC1"));

        let policy = Policy::new()
            .prefer_stable(true)
            .prefer_lowest(true)
            .prefer_dev_over_prerelease(true);
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // With prefer_dev_over_prerelease, dev is preferred over RC
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_dev);
        assert_eq!(selected[1], id_prerelease);
    }

    /// Port of Composer's testSelectNewestWithPreferredVersionPicksPreferredVersionIfAvailable
    #[test]
    fn test_select_newest_with_preferred_version_picks_preferred_if_available() {
        let mut pool = Pool::new();
        let id_a1 = pool.add_package(Package::new("a", "1.0.0"));
        let id_a2 = pool.add_package(Package::new("a", "1.1.0"));
        let id_a2b = pool.add_package(Package::new("a", "1.1.0")); // duplicate version
        let id_a3 = pool.add_package(Package::new("a", "1.2.0"));

        // Preferred version is 1.1.0.0 (normalized format)
        let policy = Policy::new()
            .prefer_stable(false)
            .prefer_lowest(false)
            .with_preferred_version("a", "1.1.0.0");
        let selected = policy.select_preferred(&pool, &[1, 2, 3, 4]);

        // Should have 1.1.0 packages first (preferred), then others
        assert_eq!(selected.len(), 4);
        // First two should be the 1.1.0 versions
        assert!(selected[..2].contains(&id_a2));
        assert!(selected[..2].contains(&id_a2b));
        // Then 1.2.0 and 1.0.0
        assert!(selected.contains(&id_a1));
        assert!(selected.contains(&id_a3));
    }

    /// Port of Composer's testSelectNewestWithPreferredVersionPicksNewestOtherwise
    #[test]
    fn test_select_newest_with_preferred_version_picks_newest_otherwise() {
        let mut pool = Pool::new();
        let id_a1 = pool.add_package(Package::new("a", "1.0.0"));
        let id_a2 = pool.add_package(Package::new("a", "1.2.0"));

        // Preferred version is 1.1.0.0 which doesn't exist
        let policy = Policy::new()
            .prefer_stable(false)
            .prefer_lowest(false)
            .with_preferred_version("a", "1.1.0.0");
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // Should fall back to version ordering (newest first)
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_a2);
        assert_eq!(selected[1], id_a1);
    }

    /// Port of Composer's testSelectNewestWithPreferredVersionPicksLowestIfPreferLowest
    #[test]
    fn test_select_newest_with_preferred_version_picks_lowest_if_prefer_lowest() {
        let mut pool = Pool::new();
        let id_a1 = pool.add_package(Package::new("a", "1.0.0"));
        let id_a2 = pool.add_package(Package::new("a", "1.2.0"));

        // Preferred version is 1.1.0.0 which doesn't exist
        let policy = Policy::new()
            .prefer_stable(false)
            .prefer_lowest(true)
            .with_preferred_version("a", "1.1.0.0");
        let selected = policy.select_preferred(&pool, &[1, 2]);

        // Should fall back to lowest (1.0.0) since prefer_lowest is true
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], id_a1);
        assert_eq!(selected[1], id_a2);
    }

    /// Port of Composer's testSelectLocalReposFirst
    /// Tests that root package aliases are preferred over other aliases
    #[test]
    fn test_select_local_repos_first() {
        use crate::package::Stability;

        let mut pool = Pool::with_minimum_stability(Stability::Dev);

        // Repo2 (lower priority) - regular packages
        let _id_a = pool.add_package_from_repo(Package::new("a", "dev-master"), Some("repo2"));
        let _id_a_alias = pool.add_alias(1, "2.1.9999999.9999999-dev", false);

        // Repo1 (higher priority) - with root package alias
        let _id_a_important = pool.add_package_from_repo(Package::new("a", "dev-feature-a"), Some("repo1"));
        let id_a_alias_important = pool.add_alias(3, "2.1.9999999.9999999-dev", true); // root package alias
        let _id_a2_important = pool.add_package_from_repo(Package::new("a", "dev-master"), Some("repo1"));
        let _id_a2_alias_important = pool.add_alias(5, "2.1.9999999.9999999-dev", false);

        pool.set_priority("repo1", 0); // higher priority
        pool.set_priority("repo2", 1); // lower priority

        let policy = Policy::new();
        // Get packages matching the alias version
        let candidates = vec![2, 4, 6]; // All the aliases

        let selected = policy.select_preferred(&pool, &candidates);

        // The root package alias from repo1 should be selected first
        assert_eq!(selected[0], id_a_alias_important);
    }
}
