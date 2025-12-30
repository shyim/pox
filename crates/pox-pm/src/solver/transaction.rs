use std::sync::Arc;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::package::{AliasPackage, Package};

#[derive(Debug, Clone, Default)]
pub struct Transaction {
    /// Operations to perform
    pub operations: Vec<Operation>,
}

/// A single operation in a transaction
#[derive(Debug, Clone)]
pub enum Operation {
    /// Install a new package
    Install(Arc<Package>),
    /// Update a package from one version to another
    Update {
        from: Arc<Package>,
        to: Arc<Package>,
    },
    /// Remove a package
    Uninstall(Arc<Package>),
    /// Mark a package as not needed (but keep it)
    MarkUnneeded(Arc<Package>),
    /// Mark an alias as installed (the alias package itself is not installed,
    /// but requirements matching the alias version are satisfied)
    MarkAliasInstalled(Arc<AliasPackage>),
    /// Mark an alias as uninstalled
    MarkAliasUninstalled(Arc<AliasPackage>),
}

impl Transaction {
    /// Create a new empty transaction
    pub fn new() -> Self {
        Self {
            operations: Vec::new(),
        }
    }

    pub fn from_packages(
        present_packages: Vec<Arc<Package>>,
        result_packages: Vec<Arc<Package>>,
        result_aliases: Vec<Arc<AliasPackage>>,
    ) -> Self {
        let mut tx = Self::new();
        tx.calculate_operations(present_packages, result_packages, result_aliases);
        tx
    }

    fn calculate_operations(
        &mut self,
        present_packages: Vec<Arc<Package>>,
        result_packages: Vec<Arc<Package>>,
        result_aliases: Vec<Arc<AliasPackage>>,
    ) {
        let mut present_package_map: HashMap<String, Arc<Package>> = HashMap::new();
        let mut remove_map: HashMap<String, Arc<Package>> = HashMap::new();

        let present_alias_map: HashMap<String, Arc<AliasPackage>> = HashMap::new();
        let mut remove_alias_map: HashMap<String, Arc<AliasPackage>> = HashMap::new();

        for package in &present_packages {
            let name_lower = package.name.to_lowercase();
            present_package_map.insert(name_lower.clone(), package.clone());
            remove_map.insert(name_lower, package.clone());
        }

        for package in &result_packages {
            let name_lower = package.name.to_lowercase();

            if let Some(present_pkg) = present_package_map.get(&name_lower) {
                if self.needs_update(present_pkg, package) {
                    self.operations.push(Operation::Update {
                        from: present_pkg.clone(),
                        to: package.clone(),
                    });
                }
                remove_map.remove(&name_lower);
            } else {
                self.operations.push(Operation::Install(package.clone()));
                remove_map.remove(&name_lower);
            }
        }

        for alias in &result_aliases {
            let alias_key = format!("{}::{}", alias.name().to_lowercase(), alias.version());
            if present_alias_map.contains_key(&alias_key) {
                remove_alias_map.remove(&alias_key);
            } else {
                self.operations.push(Operation::MarkAliasInstalled(alias.clone()));
            }
        }

        let mut remove_list: Vec<_> = remove_map.into_iter().collect();
        remove_list.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, package) in remove_list {
            self.operations.insert(0, Operation::Uninstall(package));
        }

        let mut remove_alias_list: Vec<_> = remove_alias_map.into_iter().collect();
        remove_alias_list.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, alias) in remove_alias_list {
            self.operations.push(Operation::MarkAliasUninstalled(alias));
        }

        self.move_plugins_to_front();
        self.move_uninstalls_to_front();
    }

    fn needs_update(&self, present: &Package, target: &Package) -> bool {
        if present.version != target.version {
            return true;
        }

        let present_dist_ref = present.dist.as_ref().and_then(|d| d.reference.as_ref());
        let target_dist_ref = target.dist.as_ref().and_then(|d| d.reference.as_ref());
        if present_dist_ref.is_some() && target_dist_ref.is_some() && present_dist_ref != target_dist_ref {
            return true;
        }

        let present_source_ref = present.source.as_ref().map(|s| &s.reference);
        let target_source_ref = target.source.as_ref().map(|s| &s.reference);
        if present_source_ref.is_some() && target_source_ref.is_some() && present_source_ref != target_source_ref {
            return true;
        }

        false
    }

    /// Move plugin installations to the front (after uninstalls).
    /// Plugins need to be installed before packages that depend on them.
    fn move_plugins_to_front(&mut self) {
        let mut plugins_no_deps = Vec::new();
        let mut plugins_with_deps = Vec::new();
        let mut plugin_requires: HashSet<String> = HashSet::new();
        let mut other_ops = Vec::new();

        // Process in reverse to handle dependencies correctly
        for op in self.operations.drain(..).rev() {
            let package = match &op {
                Operation::Install(pkg) => Some(pkg.clone()),
                Operation::Update { to, .. } => Some(to.clone()),
                _ => None,
            };

            if let Some(pkg) = package {
                let is_plugin = pkg.package_type == "composer-plugin"
                    || pkg.package_type == "composer-installer";

                // Check if this is a plugin or dependency of a plugin
                let names: HashSet<_> = pkg.get_names(true).into_iter().collect();
                let is_plugin_dep = !names.is_disjoint(&plugin_requires);

                if is_plugin || is_plugin_dep {
                    // Get non-platform requires
                    let requires: Vec<_> = pkg.require.keys()
                        .filter(|r| {
                            let r_lower = r.to_lowercase();
                            !r_lower.starts_with("php") && !r_lower.starts_with("ext-") && !r_lower.starts_with("lib-")
                        })
                        .map(|s| s.to_lowercase())
                        .collect();

                    if is_plugin && requires.is_empty() {
                        plugins_no_deps.insert(0, op);
                    } else {
                        plugin_requires.extend(requires);
                        plugins_with_deps.insert(0, op);
                    }
                    continue;
                }
            }
            other_ops.insert(0, op);
        }

        // Reconstruct: plugins_no_deps, plugins_with_deps, other_ops
        self.operations.extend(plugins_no_deps);
        self.operations.extend(plugins_with_deps);
        self.operations.extend(other_ops);
    }

    /// Move uninstall operations to the front.
    fn move_uninstalls_to_front(&mut self) {
        let mut uninstalls = Vec::new();
        let mut others = Vec::new();

        for op in self.operations.drain(..) {
            match &op {
                Operation::Uninstall(_) | Operation::MarkAliasUninstalled(_) => {
                    uninstalls.push(op);
                }
                _ => others.push(op),
            }
        }

        self.operations.extend(uninstalls);
        self.operations.extend(others);
    }

    /// Add an install operation
    pub fn install(&mut self, package: Arc<Package>) {
        self.operations.push(Operation::Install(package));
    }

    /// Add an update operation
    pub fn update(&mut self, from: Arc<Package>, to: Arc<Package>) {
        self.operations.push(Operation::Update { from, to });
    }

    /// Add an uninstall operation
    pub fn uninstall(&mut self, package: Arc<Package>) {
        self.operations.push(Operation::Uninstall(package));
    }

    /// Add a mark alias installed operation
    pub fn mark_alias_installed(&mut self, alias: Arc<AliasPackage>) {
        self.operations.push(Operation::MarkAliasInstalled(alias));
    }

    /// Add a mark alias uninstalled operation
    pub fn mark_alias_uninstalled(&mut self, alias: Arc<AliasPackage>) {
        self.operations.push(Operation::MarkAliasUninstalled(alias));
    }

    /// Check if the transaction is empty
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Get the number of operations
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Get all packages that will be installed (including updates)
    pub fn installs(&self) -> impl Iterator<Item = &Arc<Package>> {
        self.operations.iter().filter_map(|op| match op {
            Operation::Install(pkg) => Some(pkg),
            Operation::Update { to, .. } => Some(to),
            _ => None,
        })
    }

    /// Get all packages that will be removed (including updates)
    pub fn uninstalls(&self) -> impl Iterator<Item = &Arc<Package>> {
        self.operations.iter().filter_map(|op| match op {
            Operation::Uninstall(pkg) => Some(pkg),
            Operation::Update { from, .. } => Some(from),
            _ => None,
        })
    }

    /// Get only new installs (not updates)
    pub fn new_installs(&self) -> impl Iterator<Item = &Arc<Package>> {
        self.operations.iter().filter_map(|op| match op {
            Operation::Install(pkg) => Some(pkg),
            _ => None,
        })
    }

    /// Get only updates
    pub fn updates(&self) -> impl Iterator<Item = (&Arc<Package>, &Arc<Package>)> {
        self.operations.iter().filter_map(|op| match op {
            Operation::Update { from, to } => Some((from, to)),
            _ => None,
        })
    }

    /// Get only removals (not updates)
    pub fn removals(&self) -> impl Iterator<Item = &Arc<Package>> {
        self.operations.iter().filter_map(|op| match op {
            Operation::Uninstall(pkg) => Some(pkg),
            _ => None,
        })
    }

    /// Sort operations for proper execution order.
    /// Uninstalls first, then installs (sorted by dependencies).
    pub fn sort(&mut self) {
        // Separate operations by type
        let mut uninstalls: Vec<Operation> = Vec::new();
        let mut updates: Vec<Operation> = Vec::new();
        let mut installs: Vec<Operation> = Vec::new();
        let mut mark_unneeded: Vec<Operation> = Vec::new();
        let mut alias_ops: Vec<Operation> = Vec::new();

        for op in self.operations.drain(..) {
            match &op {
                Operation::Uninstall(_) => uninstalls.push(op),
                Operation::Update { .. } => updates.push(op),
                Operation::Install(_) => installs.push(op),
                Operation::MarkUnneeded(_) => mark_unneeded.push(op),
                Operation::MarkAliasInstalled(_) | Operation::MarkAliasUninstalled(_) => alias_ops.push(op),
            }
        }

        // Sort installs by dependency order using topological sort
        let sorted_installs = topological_sort_operations(installs);

        // Also sort updates by dependency order (using the target package)
        let sorted_updates = topological_sort_operations(updates);

        // Reconstruct operations: uninstalls first, then updates, then installs, then alias ops, then mark_unneeded
        self.operations.extend(uninstalls);
        self.operations.extend(sorted_updates);
        self.operations.extend(sorted_installs);
        self.operations.extend(alias_ops);
        self.operations.extend(mark_unneeded);
    }

    /// Get a summary of the transaction
    pub fn summary(&self) -> TransactionSummary {
        let mut summary = TransactionSummary::default();

        for op in &self.operations {
            match op {
                Operation::Install(_) => summary.installs += 1,
                Operation::Update { .. } => summary.updates += 1,
                Operation::Uninstall(_) => summary.uninstalls += 1,
                Operation::MarkUnneeded(_) => summary.mark_unneeded += 1,
                Operation::MarkAliasInstalled(_) => summary.alias_installs += 1,
                Operation::MarkAliasUninstalled(_) => summary.alias_uninstalls += 1,
            }
        }

        summary
    }

    /// Get all alias packages that will be marked as installed
    pub fn alias_installs(&self) -> impl Iterator<Item = &Arc<AliasPackage>> {
        self.operations.iter().filter_map(|op| match op {
            Operation::MarkAliasInstalled(alias) => Some(alias),
            _ => None,
        })
    }
}

/// Summary of a transaction
#[derive(Debug, Clone, Default)]
pub struct TransactionSummary {
    pub installs: usize,
    pub updates: usize,
    pub uninstalls: usize,
    pub mark_unneeded: usize,
    pub alias_installs: usize,
    pub alias_uninstalls: usize,
}

impl std::fmt::Display for TransactionSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();

        if self.installs > 0 {
            parts.push(format!("{} install(s)", self.installs));
        }
        if self.updates > 0 {
            parts.push(format!("{} update(s)", self.updates));
        }
        if self.uninstalls > 0 {
            parts.push(format!("{} removal(s)", self.uninstalls));
        }

        if parts.is_empty() {
            write!(f, "Nothing to do")
        } else {
            write!(f, "{}", parts.join(", "))
        }
    }
}

/// Sort operations using topological sort based on package dependencies.
/// Dependencies are installed before the packages that depend on them.
fn topological_sort_operations(operations: Vec<Operation>) -> Vec<Operation> {
    if operations.is_empty() {
        return operations;
    }

    // Build a map of package name -> operation index
    let mut name_to_index: HashMap<String, usize> = HashMap::new();
    let mut packages: Vec<Arc<Package>> = Vec::new();

    for (idx, op) in operations.iter().enumerate() {
        let pkg = match op {
            Operation::Install(p) => p.clone(),
            Operation::Update { to, .. } => to.clone(),
            _ => continue,
        };
        name_to_index.insert(pkg.name.to_lowercase(), idx);
        packages.push(pkg);
    }

    // Build adjacency list for dependencies
    // If A depends on B, then edge: A -> B (B must be installed before A)
    let mut in_degree: Vec<usize> = vec![0; operations.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); operations.len()];

    for (idx, pkg) in packages.iter().enumerate() {
        for (dep_name, _) in &pkg.require {
            let dep_lower = dep_name.to_lowercase();
            // Skip platform requirements
            if dep_lower == "php" || dep_lower.starts_with("ext-") || dep_lower.starts_with("lib-") {
                continue;
            }
            if let Some(&dep_idx) = name_to_index.get(&dep_lower) {
                // pkg depends on dep, so dep must be installed first
                // Edge: dep_idx -> idx (when dep is installed, it unblocks idx)
                dependents[dep_idx].push(idx);
                in_degree[idx] += 1;
            }
        }
    }

    // Kahn's algorithm for topological sort
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut result: Vec<usize> = Vec::new();

    // Start with packages that have no dependencies (in the transaction)
    for (idx, &degree) in in_degree.iter().enumerate() {
        if degree == 0 {
            queue.push_back(idx);
        }
    }

    while let Some(idx) = queue.pop_front() {
        result.push(idx);

        for &dependent_idx in &dependents[idx] {
            in_degree[dependent_idx] -= 1;
            if in_degree[dependent_idx] == 0 {
                queue.push_back(dependent_idx);
            }
        }
    }

    // If there's a cycle (result.len() != operations.len()), append remaining items
    // This shouldn't happen with valid dependency resolution, but handle gracefully
    if result.len() != operations.len() {
        let in_result: HashSet<usize> = result.iter().copied().collect();
        for idx in 0..operations.len() {
            if !in_result.contains(&idx) {
                result.push(idx);
            }
        }
    }

    // Reorder operations according to topological order
    let operations_vec: Vec<Operation> = operations;
    result.into_iter()
        .map(|idx| operations_vec[idx].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_new() {
        let tx = Transaction::new();
        assert!(tx.is_empty());
        assert_eq!(tx.len(), 0);
    }

    #[test]
    fn test_transaction_install() {
        let mut tx = Transaction::new();
        let pkg = Arc::new(Package::new("vendor/package", "1.0.0"));
        tx.install(pkg.clone());

        assert_eq!(tx.len(), 1);
        assert_eq!(tx.new_installs().count(), 1);
    }

    #[test]
    fn test_transaction_update() {
        let mut tx = Transaction::new();
        let from = Arc::new(Package::new("vendor/package", "1.0.0"));
        let to = Arc::new(Package::new("vendor/package", "2.0.0"));
        tx.update(from.clone(), to.clone());

        assert_eq!(tx.updates().count(), 1);
        assert_eq!(tx.installs().count(), 1); // Update counts as install
        assert_eq!(tx.uninstalls().count(), 1); // And uninstall
    }

    #[test]
    fn test_transaction_summary() {
        let mut tx = Transaction::new();
        tx.install(Arc::new(Package::new("a", "1.0.0")));
        tx.install(Arc::new(Package::new("b", "1.0.0")));
        tx.uninstall(Arc::new(Package::new("c", "1.0.0")));

        let summary = tx.summary();
        assert_eq!(summary.installs, 2);
        assert_eq!(summary.uninstalls, 1);
        assert_eq!(summary.updates, 0);
    }

    #[test]
    fn test_transaction_sort() {
        let mut tx = Transaction::new();
        tx.install(Arc::new(Package::new("a", "1.0.0")));
        tx.uninstall(Arc::new(Package::new("b", "1.0.0")));
        tx.install(Arc::new(Package::new("c", "1.0.0")));

        tx.sort();

        // Uninstalls should come first
        assert!(matches!(tx.operations[0], Operation::Uninstall(_)));
    }

    #[test]
    fn test_transaction_sort_by_dependencies() {
        let mut tx = Transaction::new();

        // Package c depends on b, b depends on a
        // Expected install order: a, b, c
        let pkg_a = Package::new("vendor/a", "1.0.0");
        let mut pkg_b = Package::new("vendor/b", "1.0.0");
        pkg_b.require.insert("vendor/a".to_string(), "^1.0".to_string());
        let mut pkg_c = Package::new("vendor/c", "1.0.0");
        pkg_c.require.insert("vendor/b".to_string(), "^1.0".to_string());

        // Add in wrong order
        tx.install(Arc::new(pkg_c));
        tx.install(Arc::new(pkg_a));
        tx.install(Arc::new(pkg_b));

        tx.sort();

        // Check that installs are in dependency order
        let install_names: Vec<String> = tx.operations.iter()
            .filter_map(|op| match op {
                Operation::Install(p) => Some(p.name.clone()),
                _ => None,
            })
            .collect();

        // a should come before b, b should come before c
        let a_pos = install_names.iter().position(|n| n == "vendor/a").unwrap();
        let b_pos = install_names.iter().position(|n| n == "vendor/b").unwrap();
        let c_pos = install_names.iter().position(|n| n == "vendor/c").unwrap();

        assert!(a_pos < b_pos, "a should be installed before b");
        assert!(b_pos < c_pos, "b should be installed before c");
    }

    #[test]
    fn test_transaction_sort_uninstalls_before_installs() {
        let mut tx = Transaction::new();

        tx.install(Arc::new(Package::new("vendor/new", "1.0.0")));
        tx.uninstall(Arc::new(Package::new("vendor/old", "1.0.0")));
        tx.install(Arc::new(Package::new("vendor/another", "1.0.0")));

        tx.sort();

        // Find positions of first uninstall and first install
        let first_uninstall = tx.operations.iter().position(|op| matches!(op, Operation::Uninstall(_)));
        let first_install = tx.operations.iter().position(|op| matches!(op, Operation::Install(_)));

        assert!(first_uninstall.unwrap() < first_install.unwrap(), "Uninstalls should come before installs");
    }

    #[test]
    fn test_transaction_from_packages_new_install() {
        // No present packages, one result package -> should generate Install operation
        let present = vec![];
        let result = vec![Arc::new(Package::new("vendor/a", "1.0.0"))];
        let aliases = vec![];

        let tx = Transaction::from_packages(present, result, aliases);

        assert_eq!(tx.new_installs().count(), 1);
        assert_eq!(tx.updates().count(), 0);
        assert_eq!(tx.removals().count(), 0);
    }

    #[test]
    fn test_transaction_from_packages_update() {
        // Present has v1.0.0, result has v2.0.0 -> should generate Update operation
        let present = vec![Arc::new(Package::new("vendor/a", "1.0.0"))];
        let result = vec![Arc::new(Package::new("vendor/a", "2.0.0"))];
        let aliases = vec![];

        let tx = Transaction::from_packages(present, result, aliases);

        assert_eq!(tx.new_installs().count(), 0);
        assert_eq!(tx.updates().count(), 1);
        assert_eq!(tx.removals().count(), 0);
    }

    #[test]
    fn test_transaction_from_packages_no_change() {
        // Same package version -> should generate no operations
        let present = vec![Arc::new(Package::new("vendor/a", "1.0.0"))];
        let result = vec![Arc::new(Package::new("vendor/a", "1.0.0"))];
        let aliases = vec![];

        let tx = Transaction::from_packages(present, result, aliases);

        assert_eq!(tx.new_installs().count(), 0);
        assert_eq!(tx.updates().count(), 0);
        assert_eq!(tx.removals().count(), 0);
    }

    #[test]
    fn test_transaction_from_packages_uninstall() {
        // Present has a package, result doesn't -> should generate Uninstall operation
        let present = vec![Arc::new(Package::new("vendor/a", "1.0.0"))];
        let result = vec![];
        let aliases = vec![];

        let tx = Transaction::from_packages(present, result, aliases);

        assert_eq!(tx.new_installs().count(), 0);
        assert_eq!(tx.updates().count(), 0);
        assert_eq!(tx.removals().count(), 1);
    }
}
