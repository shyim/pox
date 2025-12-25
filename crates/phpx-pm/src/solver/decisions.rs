use super::rule::Literal;
use super::pool::PackageId;

/// Tracks decisions made during SAT solving.
///
/// Each decision records:
/// - Whether a package is installed (+) or not installed (-)
/// - At what decision level it was decided
/// - Which rule caused the decision
///
/// Uses a flat Vec indexed by PackageId for O(1) lookups (like Composer).
/// The decision_map stores: 0 = undecided, >0 = installed at level N, <0 = not installed at level -N
#[derive(Debug)]
pub struct Decisions {
    /// Maps package ID to decision: 0 = undecided, >0 = installed at level, <0 = not installed at level
    /// Index is PackageId, value encodes both decision and level
    decision_map: Vec<i32>,

    /// Queue of decisions in order made [(literal, rule_id)]
    decision_queue: Vec<(Literal, Option<u32>)>,

    /// Current decision level
    level: u32,
}

impl Decisions {
    /// Create a new empty decisions tracker
    pub fn new() -> Self {
        Self {
            decision_map: Vec::new(),
            decision_queue: Vec::new(),
            level: 0,
        }
    }

    /// Create a new decisions tracker with pre-allocated capacity
    pub fn with_capacity(max_package_id: usize) -> Self {
        Self {
            decision_map: vec![0; max_package_id + 1],
            decision_queue: Vec::with_capacity(max_package_id),
            level: 0,
        }
    }

    /// Ensure the decision map can hold a package ID
    #[inline]
    fn ensure_capacity(&mut self, package_id: PackageId) {
        let id = package_id as usize;
        if id >= self.decision_map.len() {
            self.decision_map.resize(id + 1, 0);
        }
    }

    /// Get the current decision level
    #[inline]
    pub fn level(&self) -> u32 {
        self.level
    }

    /// Increment the decision level
    #[inline]
    pub fn increment_level(&mut self) {
        self.level += 1;
    }

    /// Set the decision level
    #[inline]
    pub fn set_level(&mut self, level: u32) {
        self.level = level;
    }

    /// Make a decision at the current level
    ///
    /// Returns false if this conflicts with an existing decision
    pub fn decide(&mut self, literal: Literal, rule_id: Option<u32>) -> bool {
        let package_id = literal.unsigned_abs() as PackageId;
        self.ensure_capacity(package_id);

        let id = package_id as usize;
        let existing = self.decision_map[id];

        if existing != 0 {
            // Already decided
            let was_installed = existing > 0;
            let want_installed = literal > 0;
            if was_installed != want_installed {
                return false; // Conflict
            }
            return true; // Already decided the same way
        }

        // Record decision: positive means installed, negative means not installed
        // Store level+1 so that level 0 doesn't become 0 (which means undecided)
        let level_value = (self.level + 1) as i32;
        self.decision_map[id] = if literal > 0 { level_value } else { -level_value };
        self.decision_queue.push((literal, rule_id));

        true
    }

    /// Check if a literal is satisfied by current decisions
    #[inline]
    pub fn satisfied(&self, literal: Literal) -> bool {
        let package_id = literal.unsigned_abs() as PackageId;
        let id = package_id as usize;

        if id >= self.decision_map.len() {
            return false;
        }

        let decision = self.decision_map[id];
        if decision == 0 {
            return false;
        }

        let is_installed = decision > 0;
        let want_installed = literal > 0;
        is_installed == want_installed
    }

    /// Check if a literal conflicts with current decisions
    #[inline]
    pub fn conflict(&self, literal: Literal) -> bool {
        let package_id = literal.unsigned_abs() as PackageId;
        let id = package_id as usize;

        if id >= self.decision_map.len() {
            return false;
        }

        let decision = self.decision_map[id];
        if decision == 0 {
            return false;
        }

        let is_installed = decision > 0;
        let want_installed = literal > 0;
        is_installed != want_installed
    }

    /// Check if a package has been decided (either way)
    #[inline]
    pub fn decided(&self, package_id: PackageId) -> bool {
        let id = package_id as usize;
        id < self.decision_map.len() && self.decision_map[id] != 0
    }

    /// Check if a package is undecided
    #[inline]
    pub fn undecided(&self, package_id: PackageId) -> bool {
        !self.decided(package_id)
    }

    /// Check if a package was decided to be installed
    #[inline]
    pub fn decided_install(&self, package_id: PackageId) -> bool {
        let id = package_id as usize;
        id < self.decision_map.len() && self.decision_map[id] > 0
    }

    /// Check if a package was decided to not be installed
    #[inline]
    pub fn decided_remove(&self, package_id: PackageId) -> bool {
        let id = package_id as usize;
        id < self.decision_map.len() && self.decision_map[id] < 0
    }

    /// Get the decision level for a literal/package
    #[inline]
    pub fn decision_level(&self, literal: Literal) -> Option<u32> {
        let package_id = literal.unsigned_abs() as PackageId;
        let id = package_id as usize;

        if id >= self.decision_map.len() {
            return None;
        }

        let decision = self.decision_map[id];
        if decision == 0 {
            None
        } else {
            // Subtract 1 to get back the original level (we stored level+1)
            Some(decision.unsigned_abs() - 1)
        }
    }

    /// Get the rule that caused a decision
    pub fn decision_rule(&self, literal: Literal) -> Option<u32> {
        let package_id = literal.unsigned_abs() as PackageId;

        // Find in queue (could be optimized with a separate map if needed)
        for &(lit, rule_id) in &self.decision_queue {
            if lit.unsigned_abs() as PackageId == package_id {
                return rule_id;
            }
        }
        None
    }

    /// Revert all decisions at levels > target_level
    pub fn revert_to_level(&mut self, target_level: u32) {
        // We store level+1, so target comparison needs +1 as well
        let target = (target_level + 1) as i32;

        // Clear decisions above target level
        for decision in &mut self.decision_map {
            if *decision != 0 && (decision.unsigned_abs() as i32) > target {
                *decision = 0;
            }
        }

        // Remove from queue - check directly against the map
        let decision_map = &self.decision_map;
        self.decision_queue.retain(|(literal, _)| {
            let id = literal.unsigned_abs() as usize;
            id < decision_map.len() && decision_map[id] != 0
        });

        self.level = target_level;
    }

    /// Get all packages decided to be installed
    pub fn installed_packages(&self) -> impl Iterator<Item = PackageId> + '_ {
        self.decision_map
            .iter()
            .enumerate()
            .filter(|(_, &d)| d > 0)
            .map(|(id, _)| id as PackageId)
    }

    /// Get the decision queue
    pub fn queue(&self) -> &[(Literal, Option<u32>)] {
        &self.decision_queue
    }

    /// Get decisions at a specific level
    pub fn decisions_at_level(&self, level: u32) -> Vec<Literal> {
        self.decision_queue
            .iter()
            .filter_map(|&(literal, _)| {
                if self.decision_level(literal) == Some(level) {
                    Some(literal)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get the number of decisions
    pub fn len(&self) -> usize {
        self.decision_queue.len()
    }

    /// Check if no decisions have been made
    pub fn is_empty(&self) -> bool {
        self.decision_queue.is_empty()
    }

    /// Reset all decisions
    pub fn reset(&mut self) {
        self.decision_map.fill(0);
        self.decision_queue.clear();
        self.level = 0;
    }

    /// Get a snapshot of current decisions for debugging
    pub fn snapshot(&self) -> Vec<(PackageId, bool, u32)> {
        self.decision_map
            .iter()
            .enumerate()
            .filter(|(_, &d)| d != 0)
            .map(|(id, &d)| {
                // Subtract 1 to get original level (we stored level+1)
                (id as PackageId, d > 0, d.unsigned_abs() - 1)
            })
            .collect()
    }
}

impl Default for Decisions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decisions_new() {
        let decisions = Decisions::new();
        assert_eq!(decisions.level(), 0);
        assert!(decisions.is_empty());
    }

    #[test]
    fn test_decisions_decide() {
        let mut decisions = Decisions::new();

        // Decide to install package 1
        assert!(decisions.decide(1, Some(0)));
        assert!(decisions.satisfied(1));
        assert!(!decisions.satisfied(-1));
        assert!(decisions.decided_install(1));

        // Decide to not install package 2
        assert!(decisions.decide(-2, Some(1)));
        assert!(decisions.satisfied(-2));
        assert!(!decisions.satisfied(2));
        assert!(decisions.decided_remove(2));
    }

    #[test]
    fn test_decisions_conflict() {
        let mut decisions = Decisions::new();

        decisions.decide(1, None);

        // Trying to decide opposite should fail
        assert!(!decisions.decide(-1, None));

        // Check conflict detection
        assert!(decisions.conflict(-1));
        assert!(!decisions.conflict(1));
    }

    #[test]
    fn test_decisions_levels() {
        let mut decisions = Decisions::new();
        decisions.increment_level(); // Level 1

        decisions.decide(1, None);
        assert_eq!(decisions.decision_level(1), Some(1));

        decisions.increment_level(); // Level 2
        decisions.decide(2, None);
        assert_eq!(decisions.decision_level(2), Some(2));
    }

    #[test]
    fn test_decisions_revert() {
        let mut decisions = Decisions::new();

        decisions.increment_level();
        decisions.decide(1, None);

        decisions.increment_level();
        decisions.decide(2, None);

        decisions.increment_level();
        decisions.decide(3, None);

        // Revert to level 1
        decisions.revert_to_level(1);

        assert!(decisions.decided(1));
        assert!(!decisions.decided(2));
        assert!(!decisions.decided(3));
        assert_eq!(decisions.level(), 1);
    }

    #[test]
    fn test_decisions_installed_packages() {
        let mut decisions = Decisions::new();
        decisions.decide(1, None);
        decisions.decide(-2, None);
        decisions.decide(3, None);

        let installed: Vec<_> = decisions.installed_packages().collect();
        assert_eq!(installed.len(), 2);
        assert!(installed.contains(&1));
        assert!(installed.contains(&3));
        assert!(!installed.contains(&2));
    }

    #[test]
    fn test_decisions_undecided() {
        let mut decisions = Decisions::new();
        decisions.decide(1, None);

        assert!(!decisions.undecided(1));
        assert!(decisions.undecided(2));
    }

    #[test]
    fn test_decisions_decision_rule() {
        let mut decisions = Decisions::new();
        decisions.decide(1, Some(42));
        decisions.decide(2, None);

        assert_eq!(decisions.decision_rule(1), Some(42));
        assert_eq!(decisions.decision_rule(2), None);
    }
}
