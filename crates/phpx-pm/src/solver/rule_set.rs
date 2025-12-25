use std::collections::HashMap;

use super::rule::{Rule, RuleType, Literal};

/// Collection of SAT rules organized by type.
///
/// The RuleSet manages rules with:
/// - Deduplication based on literal content
/// - Priority ordering by rule type
/// - Sequential ID assignment
#[derive(Debug)]
pub struct RuleSet {
    /// All rules indexed by ID
    rules: Vec<Rule>,

    /// Rules by type for iteration
    rules_by_type: HashMap<RuleType, Vec<u32>>,

    /// Hash map for deduplication
    rule_hashes: HashMap<u64, u32>,

    /// Next rule ID to assign
    next_id: u32,
}

impl RuleSet {
    /// Create a new empty rule set
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            rules_by_type: HashMap::new(),
            rule_hashes: HashMap::new(),
            next_id: 0,
        }
    }

    /// Add a rule to the set, returning its ID.
    /// Returns existing rule's ID if a duplicate exists.
    pub fn add(&mut self, mut rule: Rule) -> u32 {
        // Check for duplicates
        let hash = rule.literal_hash();
        if let Some(&existing_id) = self.rule_hashes.get(&hash) {
            // Verify it's actually the same rule (hash collision check)
            if let Some(existing) = self.get(existing_id) {
                if existing.equals_literals(&rule) {
                    return existing_id;
                }
            }
        }

        // Assign ID
        let id = self.next_id;
        self.next_id += 1;
        rule.set_id(id);

        // Index by type
        let rule_type = rule.rule_type();
        self.rules_by_type
            .entry(rule_type)
            .or_default()
            .push(id);

        // Store hash for deduplication
        self.rule_hashes.insert(hash, id);

        // Store rule
        self.rules.push(rule);

        id
    }

    /// Get a rule by ID
    pub fn get(&self, id: u32) -> Option<&Rule> {
        self.rules.get(id as usize)
    }

    /// Get a mutable reference to a rule by ID
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Rule> {
        self.rules.get_mut(id as usize)
    }

    /// Get all rules of a specific type
    pub fn rules_of_type(&self, rule_type: RuleType) -> impl Iterator<Item = &Rule> {
        self.rules_by_type
            .get(&rule_type)
            .into_iter()
            .flatten()
            .filter_map(move |&id| self.get(id))
    }

    /// Get all rules
    pub fn iter(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter()
    }

    /// Get all rules sorted by priority (request rules first)
    pub fn iter_by_priority(&self) -> impl Iterator<Item = &Rule> {
        let mut rules: Vec<_> = self.rules.iter().collect();
        rules.sort_by_key(|r| r.rule_type().priority());
        rules.into_iter()
    }

    /// Get assertion rules (single literal rules)
    pub fn assertions(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter().filter(|r| r.is_assertion())
    }

    /// Get the total number of rules
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Check if the rule set is empty
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Count rules by type
    pub fn count_by_type(&self, rule_type: RuleType) -> usize {
        self.rules_by_type
            .get(&rule_type)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Find rules containing a specific literal
    pub fn rules_containing(&self, literal: Literal) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|r| r.literals().contains(&literal))
            .collect()
    }

    /// Disable a rule
    pub fn disable(&mut self, id: u32) {
        if let Some(rule) = self.get_mut(id) {
            rule.disable();
        }
    }

    /// Enable a rule
    pub fn enable(&mut self, id: u32) {
        if let Some(rule) = self.get_mut(id) {
            rule.enable();
        }
    }

    /// Get statistics about the rule set
    pub fn stats(&self) -> RuleSetStats {
        let mut stats = RuleSetStats::default();
        stats.total = self.rules.len();

        for rule in &self.rules {
            match rule.rule_type() {
                RuleType::RootRequire => stats.root_require += 1,
                RuleType::Fixed => stats.fixed += 1,
                RuleType::PackageRequires => stats.requires += 1,
                RuleType::PackageConflict => stats.conflict += 1,
                RuleType::PackageSameName => stats.same_name += 1,
                RuleType::MultiConflict => stats.multi_conflict += 1,
                RuleType::PackageAlias => stats.alias += 1,
                RuleType::PackageInverseAlias => stats.inverse_alias += 1,
                RuleType::Learned => stats.learned += 1,
            }

            if rule.is_assertion() {
                stats.assertions += 1;
            }
        }

        stats
    }
}

impl Default for RuleSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about a rule set
#[derive(Debug, Default)]
pub struct RuleSetStats {
    pub total: usize,
    pub assertions: usize,
    pub root_require: usize,
    pub fixed: usize,
    pub requires: usize,
    pub conflict: usize,
    pub same_name: usize,
    pub multi_conflict: usize,
    pub alias: usize,
    pub inverse_alias: usize,
    pub learned: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_set_add() {
        let mut rules = RuleSet::new();

        let id1 = rules.add(Rule::assertion(1, RuleType::Fixed));
        let id2 = rules.add(Rule::requires(1, vec![2, 3]));

        assert_eq!(id1, 0);
        assert_eq!(id2, 1);
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn test_rule_set_deduplication() {
        let mut rules = RuleSet::new();

        let id1 = rules.add(Rule::new(vec![1, 2, 3], RuleType::PackageRequires));
        let id2 = rules.add(Rule::new(vec![3, 1, 2], RuleType::PackageRequires));

        // Same literals, different order - should deduplicate
        assert_eq!(id1, id2);
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn test_rule_set_get() {
        let mut rules = RuleSet::new();
        rules.add(Rule::assertion(5, RuleType::Fixed));

        let rule = rules.get(0).unwrap();
        assert_eq!(rule.literals(), &[5]);
    }

    #[test]
    fn test_rule_set_rules_of_type() {
        let mut rules = RuleSet::new();
        rules.add(Rule::assertion(1, RuleType::Fixed));
        rules.add(Rule::assertion(2, RuleType::Fixed));
        rules.add(Rule::requires(1, vec![3, 4]));

        let fixed: Vec<_> = rules.rules_of_type(RuleType::Fixed).collect();
        assert_eq!(fixed.len(), 2);

        let requires: Vec<_> = rules.rules_of_type(RuleType::PackageRequires).collect();
        assert_eq!(requires.len(), 1);
    }

    #[test]
    fn test_rule_set_assertions() {
        let mut rules = RuleSet::new();
        rules.add(Rule::assertion(1, RuleType::Fixed));
        rules.add(Rule::requires(1, vec![2, 3]));
        rules.add(Rule::assertion(4, RuleType::RootRequire));

        let assertions: Vec<_> = rules.assertions().collect();
        assert_eq!(assertions.len(), 2);
    }

    #[test]
    fn test_rule_set_stats() {
        let mut rules = RuleSet::new();
        rules.add(Rule::assertion(1, RuleType::Fixed));
        rules.add(Rule::requires(1, vec![2, 3]));
        rules.add(Rule::conflict(vec![2, 3]));

        let stats = rules.stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.fixed, 1);
        assert_eq!(stats.requires, 1);
        assert_eq!(stats.conflict, 1);
        assert_eq!(stats.assertions, 1);
    }

    #[test]
    fn test_rule_set_disable() {
        let mut rules = RuleSet::new();
        rules.add(Rule::assertion(1, RuleType::Fixed));

        assert!(!rules.get(0).unwrap().is_disabled());

        rules.disable(0);
        assert!(rules.get(0).unwrap().is_disabled());

        rules.enable(0);
        assert!(!rules.get(0).unwrap().is_disabled());
    }
}
