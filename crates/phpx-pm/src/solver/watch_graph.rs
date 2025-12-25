use super::rule::{Rule, Literal};
use super::rule_set::RuleSet;

/// Two-watched literals graph for efficient unit propagation.
///
/// Each non-assertion rule watches exactly 2 of its literals.
/// When a watched literal becomes false, we try to find another
/// literal to watch. This reduces propagation from O(n) to O(1) average.
#[derive(Debug)]
pub struct WatchGraph {
    /// Maps literal index -> list of (rule_id, other_watched_literal)
    /// Index is mapped from literal using literal_to_index
    watches: Vec<Vec<WatchNode>>,
}

/// A watch node linking a rule to a watched literal
#[derive(Debug, Clone, Copy)]
pub(crate) struct WatchNode {
    /// Rule ID
    rule_id: u32,
    /// The other watched literal in this rule
    other_watch: Literal,
}

impl WatchGraph {
    /// Create a new empty watch graph
    pub fn new() -> Self {
        Self {
            watches: Vec::new(),
        }
    }

    /// Convert literal to index (handles positive and negative literals)
    fn literal_to_index(literal: Literal) -> usize {
        let abs = literal.abs() as usize;
        if literal > 0 {
            abs * 2
        } else {
            abs * 2 + 1
        }
    }

    /// Get mutable reference to watches for a literal, resizing if needed
    fn get_watches_mut(&mut self, literal: Literal) -> &mut Vec<WatchNode> {
        let idx = Self::literal_to_index(literal);
        if idx >= self.watches.len() {
            self.watches.resize(idx + 1, Vec::new());
        }
        &mut self.watches[idx]
    }

    /// Build the watch graph from a rule set
    pub fn from_rules(rules: &RuleSet) -> Self {
        let mut graph = Self::new();

        for rule in rules.iter() {
            if rule.is_disabled() || rule.is_assertion() {
                continue;
            }

            graph.add_rule(rule);
        }

        graph
    }

    /// Add a rule to the watch graph
    pub fn add_rule(&mut self, rule: &Rule) {
        let literals = rule.literals();
        if literals.len() < 2 {
            return; // Assertions don't need watches
        }

        let rule_id = rule.id();

        // Multi-conflict rules watch ALL their literals for efficiency
        // When any literal becomes true (package installed), it immediately triggers a check
        if rule.is_multi_conflict() {
            // For multi-conflict, we watch all literals
            // Each watch node points to the first literal as "other" (just for the struct)
            let first = literals[0];
            for &lit in literals {
                self.get_watches_mut(lit).push(WatchNode {
                    rule_id,
                    other_watch: first, // Placeholder, multi-conflict handles this specially
                });
            }
            return;
        }

        // Standard 2-watched-literal scheme
        let watch1 = literals[0];
        let watch2 = literals[1];

        // Add watch for first literal
        self.get_watches_mut(watch1).push(WatchNode {
            rule_id,
            other_watch: watch2,
        });

        // Add watch for second literal
        self.get_watches_mut(watch2).push(WatchNode {
            rule_id,
            other_watch: watch1,
        });
    }

    /// Get rules watching a specific literal
    pub fn get_watches(&self, literal: Literal) -> &[WatchNode] {
        let idx = Self::literal_to_index(literal);
        if idx < self.watches.len() {
            &self.watches[idx]
        } else {
            &[]
        }
    }

    /// Remove a watch from a literal
    fn remove_watch(&mut self, literal: Literal, rule_id: u32) {
        let idx = Self::literal_to_index(literal);
        if idx < self.watches.len() {
            self.watches[idx].retain(|w| w.rule_id != rule_id);
        }
    }

    /// Move a watch from one literal to another
    pub fn move_watch(&mut self, rule_id: u32, from: Literal, to: Literal, other: Literal) {
        self.remove_watch(from, rule_id);
        self.get_watches_mut(to).push(WatchNode {
             rule_id,
             other_watch: other,
        });
    }
}

impl Default for WatchGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of propagating a literal
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropagateResult {
    /// Propagation successful, no conflict
    Ok,
    /// A new unit was found that must be propagated
    Unit(Literal, u32), // (literal to propagate, rule_id)
    /// A conflict was found
    Conflict(u32), // rule_id
}

/// Propagator handles unit propagation using the watch graph
#[derive(Debug)]
pub struct Propagator<'a> {
    graph: &'a mut WatchGraph,
    rules: &'a RuleSet,
}

impl<'a> Propagator<'a> {
    /// Create a new propagator
    pub fn new(graph: &'a mut WatchGraph, rules: &'a RuleSet) -> Self {
        Self { graph, rules }
    }

    /// Propagate a decided literal through the watch graph.
    ///
    /// When literal L is decided false, we need to check all rules
    /// watching L. For each rule, if the other watched literal is:
    /// - Already false: we need to find a new literal to watch
    /// - True: rule is satisfied
    /// - Undecided: potential unit propagation
    pub fn propagate<F>(&mut self, literal: Literal, mut is_satisfied: F) -> Vec<PropagateResult>
    where
        F: FnMut(Literal) -> Option<bool>, // None = undecided
    {
        let mut results = Vec::new();

        // We're propagating that `literal` is now decided
        // Rules watch the negation (when -literal becomes true, literal becomes false)
        let false_literal = -literal;

        // Get rules watching the literal that just became false
        let watches: Vec<_> = self.graph.get_watches(false_literal).to_vec();

        for watch in watches {
            let Some(rule) = self.rules.get(watch.rule_id) else {
                continue;
            };

            if rule.is_disabled() {
                continue;
            }

            // Multi-conflict rules are handled differently
            // They watch all literals and trigger when a package is INSTALLED (literal becomes true)
            // For multi-conflict: literals are [-A, -B, -C, ...] meaning "not all of A,B,C can be installed"
            // When we decide +A (install A), the literal -A becomes false, and we need to propagate
            // that all other packages in the conflict must NOT be installed
            if rule.is_multi_conflict() {
                let result = self.propagate_multi_conflict(rule, false_literal, &mut is_satisfied, &mut results);
                if result != PropagateResult::Ok {
                    results.push(result);
                }
                continue;
            }

            let other = watch.other_watch;

            // Check if other watched literal satisfies the rule
            match is_satisfied(other) {
                Some(true) => {
                    // Rule is satisfied by other literal
                    continue;
                }
                Some(false) => {
                    // Both watched literals are false, need to find new watch or conflict
                    let result = self.find_new_watch(rule, false_literal, other, &mut is_satisfied);
                    if result != PropagateResult::Ok {
                        results.push(result);
                    }
                }
                None => {
                    // Other is undecided - check if this is a unit clause
                    let result = self.check_unit(rule, false_literal, other, &mut is_satisfied);
                    if result != PropagateResult::Ok {
                        results.push(result);
                    }
                }
            }
        }

        results
    }

    /// Propagate a multi-conflict rule.
    ///
    /// Multi-conflict rules represent "at most one of these packages can be installed".
    /// When one package is installed (making its negative literal false), all other
    /// packages must NOT be installed.
    fn propagate_multi_conflict<F>(
        &mut self,
        rule: &Rule,
        false_literal: Literal,
        is_satisfied: &mut F,
        results: &mut Vec<PropagateResult>,
    ) -> PropagateResult
    where
        F: FnMut(Literal) -> Option<bool>,
    {
        let literals = rule.literals();

        // false_literal is the one that just became false (meaning a package was installed)
        // All other literals in the rule must now be true (packages not installed)
        // Or there's a conflict if any other is already false (another package also installed)

        for &lit in literals {
            if lit == false_literal {
                continue;
            }

            match is_satisfied(lit) {
                Some(true) => {
                    // This literal is already satisfied (package not installed), good
                    continue;
                }
                Some(false) => {
                    // Another package is also installed - conflict!
                    return PropagateResult::Conflict(rule.id());
                }
                None => {
                    // Undecided - must propagate that this package cannot be installed
                    // The literal is negative (-pkg), so to satisfy it, the package must not be installed
                    results.push(PropagateResult::Unit(lit, rule.id()));
                }
            }
        }

        PropagateResult::Ok
    }

    /// Try to find a new literal to watch
    fn find_new_watch<F>(
        &mut self,
        rule: &Rule,
        false_literal: Literal,
        other_false: Literal,
        is_satisfied: &mut F,
    ) -> PropagateResult
    where
        F: FnMut(Literal) -> Option<bool>,
    {
        let literals = rule.literals();

        // Look for an unwatched literal that isn't false
        for &lit in literals {
            if lit == false_literal || lit == other_false {
                continue;
            }

            match is_satisfied(lit) {
                Some(true) => {
                    // Found a true literal - rule is satisfied
                    // Move watch to this literal
                    self.graph.move_watch(rule.id(), false_literal, lit, other_false);
                    return PropagateResult::Ok;
                }
                None => {
                    // Found an undecided literal - move watch to it
                    self.graph.move_watch(rule.id(), false_literal, lit, other_false);
                    return PropagateResult::Ok;
                }
                Some(false) => {
                    // This literal is also false, try next
                    continue;
                }
            }
        }

        // All literals are false - conflict!
        PropagateResult::Conflict(rule.id())
    }

    /// Check if a rule with one false and one undecided watched literal is a unit
    fn check_unit<F>(
        &mut self,
        rule: &Rule,
        false_literal: Literal,
        undecided: Literal,
        is_satisfied: &mut F,
    ) -> PropagateResult
    where
        F: FnMut(Literal) -> Option<bool>,
    {
        let literals = rule.literals();

        // Check all non-watched literals
        for &lit in literals {
            if lit == false_literal || lit == undecided {
                continue;
            }

            match is_satisfied(lit) {
                Some(true) => {
                    // Found a true literal - rule is satisfied
                    // Move watch to this literal
                    self.graph.move_watch(rule.id(), false_literal, lit, undecided);
                    return PropagateResult::Ok;
                }
                None => {
                    // Found another undecided literal - not a unit clause yet
                    // Move watch from false_literal to this undecided
                    self.graph.move_watch(rule.id(), false_literal, lit, undecided);
                    return PropagateResult::Ok;
                }
                Some(false) => {
                    continue;
                }
            }
        }

        // All other literals are false - undecided must become true (unit propagation)
        PropagateResult::Unit(undecided, rule.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::rule::RuleType;

    #[test]
    fn test_watch_graph_add_rule() {
        let mut graph = WatchGraph::new();

        let rule = Rule::new(vec![1, 2, 3], RuleType::PackageRequires);
        let mut rule = rule;
        rule.set_id(0);
        graph.add_rule(&rule);

        // Should have watches on literals 1 and 2
        assert_eq!(graph.get_watches(1).len(), 1);
        assert_eq!(graph.get_watches(2).len(), 1);
        assert_eq!(graph.get_watches(3).len(), 0);
    }

    #[test]
    fn test_watch_graph_from_rules() {
        let mut rules = RuleSet::new();
        rules.add(Rule::new(vec![1, 2, 3], RuleType::PackageRequires));
        rules.add(Rule::new(vec![1, 4, 5], RuleType::PackageRequires));
        rules.add(Rule::assertion(6, RuleType::Fixed)); // Should be ignored

        let graph = WatchGraph::from_rules(&rules);

        // Literal 1 is watched by both non-assertion rules
        assert_eq!(graph.get_watches(1).len(), 2);
    }

    #[test]
    fn test_watch_graph_move_watch() {
        let mut graph = WatchGraph::new();

        let mut rule = Rule::new(vec![1, 2, 3], RuleType::PackageRequires);
        rule.set_id(0);
        graph.add_rule(&rule);

        // Move watch from 1 to 3
        graph.move_watch(0, 1, 3, 2);

        assert_eq!(graph.get_watches(1).len(), 0);
        assert_eq!(graph.get_watches(3).len(), 1);
    }

    #[test]
    fn test_propagator_unit() {
        let mut rules = RuleSet::new();
        // Rule: (-1 | 2 | 3) = if 1 then 2 or 3
        rules.add(Rule::new(vec![-1, 2, 3], RuleType::PackageRequires));

        let mut graph = WatchGraph::from_rules(&rules);

        // Satisfy literal 1 (makes -1 false)
        // If we also say 3 is false, then 2 must be true
        let mut propagator = Propagator::new(&mut graph, &rules);
        let results = propagator.propagate(1, |lit| {
            match lit {
                -1 => Some(false), // 1 is installed, so -1 is false
                3 => Some(false),  // 3 is not installed
                2 => None,         // 2 is undecided
                _ => None,
            }
        });

        // Should find unit propagation for literal 2
        assert!(results.iter().any(|r| matches!(r, PropagateResult::Unit(2, _))));
    }

    #[test]
    fn test_propagator_conflict() {
        let mut rules = RuleSet::new();
        // Rule: (-1 | 2) = if 1 then 2
        rules.add(Rule::new(vec![-1, 2], RuleType::PackageRequires));

        let mut graph = WatchGraph::from_rules(&rules);

        // Both literals are false = conflict
        let mut propagator = Propagator::new(&mut graph, &rules);
        let results = propagator.propagate(1, |lit| {
            match lit {
                -1 => Some(false), // 1 is installed
                2 => Some(false),  // 2 is not installed
                _ => None,
            }
        });

        // Should find conflict
        assert!(results.iter().any(|r| matches!(r, PropagateResult::Conflict(_))));
    }

    #[test]
    fn test_propagator_satisfied() {
        let mut rules = RuleSet::new();
        // Rule: (-1 | 2 | 3) = if 1 then 2 or 3
        rules.add(Rule::new(vec![-1, 2, 3], RuleType::PackageRequires));

        let mut graph = WatchGraph::from_rules(&rules);

        // If 2 is already true, rule is satisfied
        let mut propagator = Propagator::new(&mut graph, &rules);
        let results = propagator.propagate(1, |lit| {
            match lit {
                -1 => Some(false),
                2 => Some(true), // Already satisfied
                _ => None,
            }
        });

        // Should not find any units or conflicts
        assert!(results.is_empty() || results.iter().all(|r| *r == PropagateResult::Ok));
    }
}
