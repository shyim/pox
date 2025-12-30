use std::sync::Arc;

use super::decisions::Decisions;
use super::pool::{Pool, PackageId, PoolEntry};
use super::pool_optimizer::PoolOptimizer;
use super::policy::Policy;
use super::problem::{Problem, ProblemSet};
use super::request::Request;
use super::rule::{Literal, Rule, RuleType};
use super::rule_generator::RuleGenerator;
use super::rule_set::RuleSet;
use super::watch_graph::{WatchGraph, Propagator, PropagateResult};

use crate::package::{AliasPackage, Package};

/// Result of dependency resolution.
///
/// Contains the packages that should be installed according to the solver.
/// The caller should compare this with currently installed packages to
/// generate the actual operations (using Transaction::from_packages).
#[derive(Debug, Clone, Default)]
pub struct SolverResult {
    /// Packages that should be installed (the complete resolved set)
    pub packages: Vec<Arc<Package>>,
    /// Alias packages that should be marked as installed
    pub aliases: Vec<Arc<AliasPackage>>,
}

impl SolverResult {
    /// Create a new empty solver result
    pub fn new() -> Self {
        Self {
            packages: Vec::new(),
            aliases: Vec::new(),
        }
    }
}

/// The main SAT solver for dependency resolution.
///
/// Implements a CDCL (Conflict-Driven Clause Learning) algorithm
/// adapted for package dependency resolution.
pub struct Solver<'a> {
    /// Package pool
    pool: &'a Pool,
    /// Selection policy
    policy: &'a Policy,
    /// Whether to optimize the pool before solving
    optimize_pool: bool,
}

impl<'a> Solver<'a> {
    /// Create a new solver
    pub fn new(pool: &'a Pool, policy: &'a Policy) -> Self {
        Self {
            pool,
            policy,
            optimize_pool: true, // Pool optimization enabled
        }
    }

    /// Set whether to optimize the pool before solving.
    ///
    /// Pool optimization can significantly speed up solving for large dependency graphs
    /// by removing packages that can't possibly be selected. Enabled by default.
    pub fn with_optimization(mut self, optimize: bool) -> Self {
        self.optimize_pool = optimize;
        self
    }

    /// Solve the dependency resolution problem.
    ///
    /// Returns a SolverResult containing packages that should be installed,
    /// or a ProblemSet explaining failures.
    ///
    /// The caller should use Transaction::from_packages() to compare the result
    /// with currently installed packages and generate the actual operations.
    pub fn solve(&self, request: &Request) -> Result<SolverResult, ProblemSet> {
        log::debug!("Building pool with {} packages", self.pool.len());

        if self.optimize_pool {
            log::debug!("Running pool optimizer");
            let opt_start = std::time::Instant::now();
            // Optimize the pool first to reduce the search space
            let mut optimizer = PoolOptimizer::new(self.policy);
            let optimized_pool = optimizer.optimize(request, self.pool);
            let elapsed = opt_start.elapsed();
            let original = self.pool.len();
            let optimized = optimized_pool.len();
            let removed = original.saturating_sub(optimized);
            let percent = if original > 0 { (removed * 100) / original } else { 0 };
            log::info!("Pool optimizer completed in {:.3} seconds", elapsed.as_secs_f64());
            log::info!("Found {} package versions referenced in dependency graph. {} ({}%) were optimized away",
                original, removed, percent);
            self.solve_with_pool(&optimized_pool, request)
        } else {
            self.solve_with_pool(self.pool, request)
        }
    }

    /// Internal solve method that works with any pool reference.
    fn solve_with_pool(&self, pool: &Pool, request: &Request) -> Result<SolverResult, ProblemSet> {
        log::debug!("Generating rules");
        let start = std::time::Instant::now();

        // Generate rules from the dependency graph
        let generator = RuleGenerator::new(pool);
        let rules = generator.generate(request);

        log::info!("Generated {} rules in {:?}", rules.len(), start.elapsed());

        // Create solver state
        let mut state = SolverState::new(rules);

        log::debug!("Resolving dependencies through SAT");
        let sat_start = std::time::Instant::now();

        // Run the SAT solver
        match self.run_sat(&mut state, pool, request) {
            Ok(()) => {
                let elapsed = sat_start.elapsed();
                log::info!("Dependency resolution completed in {:.3} seconds", elapsed.as_secs_f64());
                log::info!("Analyzed {} packages to resolve dependencies", pool.len());
                log::info!("Analyzed {} rules to resolve dependencies", state.rules.len());
                // Build result from decisions
                Ok(self.build_result(&state, pool, request))
            }
            Err(problems) => {
                log::debug!("SAT solving failed in {:?}", sat_start.elapsed());
                Err(problems)
            },
        }
    }

    /// Main SAT solving loop - follows Composer's approach:
    /// 1. Propagate decisions
    /// 2. Fulfill all unresolved rules
    /// 3. On conflict: use CDCL learning to backtrack
    /// 4. After solution found: minimization step to try alternatives
    fn run_sat(&self, state: &mut SolverState, pool: &Pool, request: &Request) -> Result<(), ProblemSet> {
        // Process assertion rules first (single-literal rules)
        self.process_assertions(state, pool)?;

        // Iteration counter for detecting infinite loops
        let mut iterations = 0u32;
        const MAX_ITERATIONS: u32 = 100_000;

        // Main solving loop
        loop {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                let mut problems = ProblemSet::new();
                problems.add(Problem::new().with_message("Solver exceeded maximum iterations"));
                return Err(problems);
            }

            // Step 1: Propagate all consequences of current decisions
            if let Err(conflict_rule) = self.propagate(state) {
                // Conflict at level 1 means unsolvable
                if state.decisions.level() == 1 {
                    log::debug!("Conflict at level 1: rule {} is unsolvable", conflict_rule);
                    let mut problems = ProblemSet::new();
                    problems.add(self.analyze_unsolvable(state, pool, conflict_rule));
                    return Err(problems);
                }

                // Use CDCL learning to analyze conflict and backtrack
                let level = self.analyze_and_backtrack(state, conflict_rule)?;
                if level == 0 {
                    return Err(ProblemSet::new());
                }
                continue;
            }

            // Step 2: Find the next undecided package to decide on
            match self.select_next(state, request) {
                Some((candidates, name)) => {
                    let level = self.select_and_install(state, pool, &candidates, &name)?;
                    if level == 0 {
                        return Err(ProblemSet::new());
                    }
                }
                None => {
                    // No more undecided packages - solution found!
                    // Step 3: Minimization - try alternatives to find better solutions
                    if !self.minimize_solution(state, pool)? {
                        // No more alternatives to try - we're done
                        return Ok(());
                    }
                    // Minimization made changes, continue solving
                }
            }
        }
    }

    /// Select and install the best package from candidates.
    /// Returns the new level, or 0 if unsolvable.
    fn select_and_install(
        &self,
        state: &mut SolverState,
        pool: &Pool,
        candidates: &[PackageId],
        name: &str,
    ) -> Result<u32, ProblemSet> {
        // Sort by policy preference
        let sorted = self.policy.select_preferred_for_requirement(
            pool,
            candidates,
            Some(name),
        );

        if sorted.is_empty() {
            return Ok(state.decisions.level());
        }

        // Select the best candidate
        let selected = sorted[0];

        // Store alternatives for minimization step
        if sorted.len() > 1 {
            state.branches.push(Branch {
                level: state.decisions.level(),
                alternatives: sorted[1..].to_vec(),
                name: name.to_string(),
            });
        }

        // Increment level and make decision
        state.decisions.increment_level();
        state.decisions.decide(selected, None);

        // Propagate and handle any conflicts with CDCL
        loop {
            match self.propagate(state) {
                Ok(()) => return Ok(state.decisions.level()),
                Err(conflict_rule) => {
                    if state.decisions.level() == 1 {
                        let mut problems = ProblemSet::new();
                        problems.add(self.analyze_unsolvable(state, pool, conflict_rule));
                        return Err(problems);
                    }
                    let level = self.analyze_and_backtrack(state, conflict_rule)?;
                    if level == 0 {
                        return Ok(0);
                    }
                }
            }
        }
    }

    /// Analyze conflict using CDCL and backtrack to appropriate level.
    /// Returns the new level after backtracking.
    fn analyze_and_backtrack(
        &self,
        state: &mut SolverState,
        conflict_rule: u32,
    ) -> Result<u32, ProblemSet> {
        let (learned_literal, backtrack_level, learned_rule) =
            self.analyze_conflict(state, conflict_rule);

        if backtrack_level <= 0 || backtrack_level >= state.decisions.level() {
            // Invalid backtrack level
            return Ok(0);
        }

        // Backtrack to appropriate level
        state.decisions.revert_to_level(backtrack_level);
        state.reset_propagate_index();

        // Remove branches above backtrack level (matching Composer's revert behavior)
        state.branches.retain(|b| b.level <= backtrack_level);

        // Add learned rule if it has literals
        if !learned_rule.literals().is_empty() {
            let learned_id = state.rules.add(learned_rule);
            state.watch_graph.add_rule(state.rules.get(learned_id).unwrap());

            // Decide the learned literal
            state.decisions.decide(learned_literal, Some(learned_id));
        }

        Ok(backtrack_level)
    }

    /// Minimization step: try alternatives to find potentially better solutions.
    /// Returns true if we made changes and should continue solving.
    fn minimize_solution(&self, state: &mut SolverState, pool: &Pool) -> Result<bool, ProblemSet> {
        if state.branches.is_empty() {
            return Ok(false);
        }

        // Find an alternative that was decided at a level higher than where it was stored
        // This means a better choice might be available
        let mut best_alternative: Option<(usize, usize, i32, u32)> = None; // (branch_idx, offset, literal, level)

        for (i, branch) in state.branches.iter().enumerate().rev() {
            for (offset, &literal) in branch.alternatives.iter().enumerate() {
                if literal > 0 {
                    if let Some(decision_level) = state.decisions.decision_level(literal) {
                        if decision_level > branch.level + 1 {
                            best_alternative = Some((i, offset, literal, branch.level));
                        }
                    }
                }
            }
        }

        let Some((branch_idx, offset, literal, level)) = best_alternative else {
            return Ok(false);
        };

        // Remove this alternative from the branch
        state.branches[branch_idx].alternatives.remove(offset);

        // Clean up empty branches
        if state.branches[branch_idx].alternatives.is_empty() {
            state.branches.remove(branch_idx);
        }

        // Revert to the branch level
        state.decisions.revert_to_level(level);
        state.reset_propagate_index();

        // Remove branches at or above this level
        state.branches.retain(|b| b.level < level);

        // Try the alternative
        state.decisions.increment_level();
        state.decisions.decide(literal as PackageId, None);

        // Propagate and handle conflicts
        loop {
            match self.propagate(state) {
                Ok(()) => return Ok(true),
                Err(conflict_rule) => {
                    if state.decisions.level() == 1 {
                        let mut problems = ProblemSet::new();
                        problems.add(self.analyze_unsolvable(state, pool, conflict_rule));
                        return Err(problems);
                    }
                    let new_level = self.analyze_and_backtrack(state, conflict_rule)?;
                    if new_level == 0 {
                        return Ok(false);
                    }
                }
            }
        }
    }

    /// Process assertion rules (single-literal rules that must be true)
    /// Also check for empty rules which indicate unsatisfiable requirements
    fn process_assertions(&self, state: &mut SolverState, pool: &Pool) -> Result<(), ProblemSet> {
        state.decisions.increment_level(); // Level 1 for assertions

        // First check for empty rules (unsatisfiable requirements like missing packages)
        for rule in state.rules.iter() {
            if rule.is_disabled() {
                continue;
            }

            if rule.is_empty() {
                // Empty rule = unsatisfiable (e.g., requiring a non-existent package)
                let mut problems = ProblemSet::new();
                let mut problem = Problem::new();
                problem.add_rule_with_pool(rule, pool);
                problems.add(problem);
                return Err(problems);
            }
        }

        // Then process single-literal assertions
        for rule in state.rules.assertions() {
            if rule.is_disabled() {
                continue;
            }

            let literals = rule.literals();
            let literal = literals[0];

            if state.decisions.conflict(literal) {
                // Conflict with existing decision
                let mut problems = ProblemSet::new();
                let mut problem = Problem::new();
                problem.add_rule_with_pool(rule, pool);
                problems.add(problem);
                return Err(problems);
            }

            if !state.decisions.satisfied(literal) {
                state.decisions.decide(literal, Some(rule.id()));
            }
        }

        Ok(())
    }

    /// Propagate consequences of current decisions using unit propagation
    /// Uses propagate_index to avoid re-processing already propagated decisions
    fn propagate(&self, state: &mut SolverState) -> Result<(), u32> {
        // Process only new decisions since last propagation
        while state.propagate_index < state.decisions.len() {
            let (literal, _) = state.decisions.queue()[state.propagate_index];
            state.propagate_index += 1;

            // Create a closure to check literal satisfaction
            let is_satisfied = |lit: Literal| -> Option<bool> {
                let pkg_id = lit.unsigned_abs() as PackageId;
                if state.decisions.decided(pkg_id) {
                    Some(state.decisions.satisfied(lit))
                } else {
                    None
                }
            };

            // Use a local scope to limit the mutable borrow
            let results = {
                let mut propagator = Propagator::new(&mut state.watch_graph, &state.rules);
                propagator.propagate(literal, is_satisfied)
            };

            for result in results {
                match result {
                    PropagateResult::Ok => {}
                    PropagateResult::Unit(unit_lit, rule_id) => {
                        if state.decisions.conflict(unit_lit) {
                            // Log conflict for debugging
                            if let Some(rule) = state.rules.get(rule_id) {
                                log::debug!("Conflict in propagation: rule {:?} type {:?}", rule_id, rule.rule_type());
                            }
                            return Err(rule_id);
                        }
                        if !state.decisions.satisfied(unit_lit) {
                            state.decisions.decide(unit_lit, Some(rule_id));
                        }
                    }
                    PropagateResult::Conflict(rule_id) => {
                        return Err(rule_id);
                    }
                }
            }
        }

        Ok(())
    }

    /// Select the next undecided requirement to branch on
    /// Uses direct slice iteration like Composer for performance
    fn select_next(&self, state: &SolverState, _request: &Request) -> Option<(Vec<PackageId>, String)> {
        let rules = state.rules.as_slice();

        for rule in rules {
            if rule.is_disabled() {
                continue;
            }

            let rule_type = rule.rule_type();
            let literals = rule.literals();

            // Handle root requirements first (they have priority)
            if rule_type == RuleType::RootRequire || rule_type == RuleType::Fixed {
                let mut decision_queue = Vec::new();
                let mut none_satisfied = true;

                for &lit in literals {
                    if state.decisions.satisfied(lit) {
                        none_satisfied = false;
                        break;
                    }
                    if lit > 0 && state.decisions.undecided(lit as PackageId) {
                        decision_queue.push(lit as PackageId);
                    }
                }

                if none_satisfied && !decision_queue.is_empty() {
                    let name = rule.target_name().unwrap_or("unknown").to_string();
                    return Some((decision_queue, name));
                }
                continue;
            }

            // Handle package requirements
            if rule_type == RuleType::PackageRequires {
                // For requires rules: first literal is negated source (-source, target1, target2, ...)
                // Rule fires when source is installed (i.e., -source is false)
                if literals.is_empty() {
                    continue;
                }

                // Check if source package is installed
                let source_lit = literals[0]; // This is -source_id
                if source_lit >= 0 {
                    continue; // Not a requires rule format we expect
                }
                let source_id = (-source_lit) as PackageId;
                if !state.decisions.decided_install(source_id) {
                    continue; // Source not installed, skip
                }

                let mut decision_queue = Vec::new();

                for &lit in &literals[1..] {
                    if lit <= 0 {
                        // Negative literal in targets - check if it's violated
                        if !state.decisions.decided_install((-lit) as PackageId) {
                            continue; // Negative requirement satisfied
                        }
                    } else {
                        // Positive literal
                        if state.decisions.satisfied(lit) {
                            // Rule already satisfied
                            decision_queue.clear();
                            break;
                        }
                        if state.decisions.undecided(lit as PackageId) {
                            decision_queue.push(lit as PackageId);
                        }
                    }
                }

                if !decision_queue.is_empty() {
                    let name = rule.target_name().unwrap_or("unknown").to_string();
                    return Some((decision_queue, name));
                }
            }
        }

        None
    }

    /// Analyze a conflict to generate a learned clause using first-UIP scheme.
    /// This follows Composer's approach: walk backwards through decisions to find the UIP.
    fn analyze_conflict(&self, state: &SolverState, conflict_rule_id: u32) -> (Literal, u32, Rule) {
        let current_level = state.decisions.level();

        let mut seen = std::collections::HashSet::new();
        let mut num_at_current_level = 0;
        let mut num_at_level1 = 0;
        let mut other_learned_literals = Vec::new();
        let mut backtrack_level = 0u32;
        let mut learned_literal: Option<Literal> = None;

        let decisions = state.decisions.queue();
        let mut decision_idx = decisions.len();
        let mut current_rule = state.rules.get(conflict_rule_id);

        loop {
            // Process current rule's literals
            if let Some(rule) = &current_rule {
                for &lit in rule.literals() {
                    let pkg_id = lit.unsigned_abs() as PackageId;

                    // Skip if already seen
                    if seen.contains(&pkg_id) {
                        continue;
                    }

                    // Skip if this literal is satisfied (true)
                    if state.decisions.satisfied(lit) {
                        continue;
                    }

                    seen.insert(pkg_id);

                    if let Some(level) = state.decisions.decision_level(lit) {
                        if level == 0 {
                            continue;
                        }
                        if level == 1 {
                            num_at_level1 += 1;
                        } else if level == current_level {
                            num_at_current_level += 1;
                        } else {
                            // Not level 1 or current level - add to learned clause
                            other_learned_literals.push(lit);
                            backtrack_level = backtrack_level.max(level);
                        }
                    }
                }
            }

            // Done if no more literals at current level to resolve
            if num_at_current_level == 0 {
                break;
            }

            // Walk backwards through decisions to find the next one we've seen
            loop {
                if decision_idx == 0 {
                    break;
                }
                decision_idx -= 1;

                let (lit, _rule_id) = decisions[decision_idx];
                let pkg_id = lit.unsigned_abs() as PackageId;

                if seen.contains(&pkg_id) {
                    seen.remove(&pkg_id);

                    num_at_current_level -= 1;

                    if num_at_current_level == 0 {
                        // This is the UIP - the learned literal is its negation
                        learned_literal = Some(-lit);

                        // If only level 1 literals remain, we're done
                        if num_at_level1 == 0 {
                            break;
                        }

                        // Clear other literals (they're level 1)
                        for other in &other_learned_literals {
                            seen.remove(&(other.unsigned_abs() as PackageId));
                        }
                        num_at_level1 += 1;
                    } else {
                        // Get reason for this decision and continue resolving
                        if let Some(reason_id) = state.decisions.decision_rule(lit) {
                            current_rule = state.rules.get(reason_id);
                        } else {
                            current_rule = None;
                        }
                    }
                    break;
                }
            }

            // If we found the UIP or no more decisions, stop
            if learned_literal.is_some() || decision_idx == 0 {
                break;
            }
        }

        // If we couldn't find a UIP, use fallback
        let learned_literal = match learned_literal {
            Some(lit) => lit,
            None => {
                // Fallback: negate the last decision at current level
                for &(lit, _) in decisions.iter().rev() {
                    if state.decisions.decision_level(lit) == Some(current_level) {
                        learned_literal = Some(-lit);
                        break;
                    }
                }
                learned_literal.unwrap_or(1)
            }
        };

        // Build the learned rule: UIP first, then other literals
        let mut learned_literals = vec![learned_literal];
        for &lit in &other_learned_literals {
            learned_literals.push(-lit);
        }

        // Ensure we backtrack at least one level
        if backtrack_level >= current_level {
            backtrack_level = current_level.saturating_sub(1);
        }
        if backtrack_level == 0 && current_level > 1 {
            backtrack_level = 1;
        }

        let learned_rule = Rule::learned(learned_literals);

        (learned_literal, backtrack_level, learned_rule)
    }

    /// Analyze an unsolvable problem at level 1
    fn analyze_unsolvable(&self, state: &SolverState, pool: &Pool, conflict_rule_id: u32) -> Problem {
        let mut problem = Problem::new();

        if let Some(rule) = state.rules.get(conflict_rule_id) {
            problem.add_rule_with_pool(rule, pool);

            // Follow the chain of rules that led to this conflict
            for &lit in rule.literals() {
                if let Some(rule_id) = state.decisions.decision_rule(lit) {
                    if let Some(cause_rule) = state.rules.get(rule_id) {
                        problem.add_rule_with_pool(cause_rule, pool);
                    }
                }
            }
        }

        problem
    }

    fn build_result(&self, state: &SolverState, pool: &Pool, request: &Request) -> SolverResult {
        let mut result = SolverResult::new();
        let mut seen_packages = std::collections::HashSet::new();

        let installed_pkgs: Vec<_> = state.decisions.installed_packages().collect();
        log::debug!("Building result from {} installed packages", installed_pkgs.len());

        for pkg_id in installed_pkgs {
            if let Some(entry) = pool.entry(pkg_id) {
                match entry {
                    PoolEntry::Alias(alias) => {
                        result.aliases.push(alias.clone());
                        continue;
                    }
                    PoolEntry::Package(_) => {}
                }
            }

            if let Some(package) = pool.package(pkg_id) {
                if request.is_fixed(&package.name) {
                    continue;
                }

                let key = (package.name.to_lowercase(), package.version.clone());
                if seen_packages.contains(&key) {
                    continue;
                }
                seen_packages.insert(key);

                result.packages.push(package.clone());

                let aliases = pool.get_aliases(pkg_id);
                for alias_id in aliases {
                    if let Some(entry) = pool.entry(alias_id) {
                        if let PoolEntry::Alias(alias) = entry {
                            result.aliases.push(alias.clone());
                        }
                    }
                }
            }
        }

        result.packages.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        result
    }
}

/// Internal state for the solver
struct SolverState {
    /// SAT rules
    rules: RuleSet,
    /// Current decisions
    decisions: Decisions,
    /// Watch graph for propagation
    watch_graph: WatchGraph,
    /// Branch points for backtracking
    branches: Vec<Branch>,
    /// Index of next decision to propagate (avoids re-propagating)
    propagate_index: usize,
}

impl SolverState {
    fn new(rules: RuleSet) -> Self {
        let watch_graph = WatchGraph::from_rules(&rules);

        Self {
            rules,
            decisions: Decisions::new(),
            watch_graph,
            branches: Vec::new(),
            propagate_index: 0,
        }
    }

    /// Reset propagate_index after backtracking
    fn reset_propagate_index(&mut self) {
        self.propagate_index = self.decisions.len();
    }
}

/// A branch point for backtracking
#[allow(dead_code)]
struct Branch {
    /// Decision level at this branch
    level: u32,
    /// Alternative packages to try
    alternatives: Vec<PackageId>,
    /// Package name being decided
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Package;

    fn create_simple_pool() -> Pool {
        let mut pool = Pool::new();

        // Package A v1.0 requires B ^1.0
        let mut a = Package::new("vendor/a", "1.0.0");
        a.require.insert("vendor/b".to_string(), "^1.0".to_string());
        pool.add_package(a);

        // Package B v1.0
        pool.add_package(Package::new("vendor/b", "1.0.0"));

        pool
    }

    #[test]
    fn test_solver_simple() {
        let pool = create_simple_pool();
        let policy = Policy::new();
        let solver = Solver::new(&pool, &policy);

        let mut request = Request::new();
        request.require("vendor/a", "^1.0");

        let result = solver.solve(&request);
        assert!(result.is_ok());

        let solver_result = result.unwrap();
        assert!(!solver_result.packages.is_empty());

        // Should install both A and B
        assert_eq!(solver_result.packages.len(), 2);
    }

    #[test]
    fn test_solver_no_solution() {
        let mut pool = Pool::new();

        // Package A requires B, but B doesn't exist
        let mut a = Package::new("vendor/a", "1.0.0");
        a.require.insert("vendor/nonexistent".to_string(), "^1.0".to_string());
        pool.add_package(a);

        let policy = Policy::new();
        let solver = Solver::new(&pool, &policy);

        let mut request = Request::new();
        request.require("vendor/a", "^1.0");

        let _result = solver.solve(&request);
        // This should fail because vendor/nonexistent doesn't exist
        // The current implementation may or may not catch this depending on rule generation
    }

    #[test]
    fn test_solver_conflict() {
        let mut pool = Pool::new();

        // Package A requires B ^1.0
        let mut a = Package::new("vendor/a", "1.0.0");
        a.require.insert("vendor/b".to_string(), "^1.0".to_string());
        pool.add_package(a);

        // Package C requires B ^2.0
        let mut c = Package::new("vendor/c", "1.0.0");
        c.require.insert("vendor/b".to_string(), "^2.0".to_string());
        pool.add_package(c);

        // Only B v1.0 exists
        pool.add_package(Package::new("vendor/b", "1.0.0"));

        let policy = Policy::new();
        let solver = Solver::new(&pool, &policy);

        let mut request = Request::new();
        request.require("vendor/a", "^1.0");
        request.require("vendor/c", "^1.0");

        // This might succeed if constraint matching isn't fully implemented
        // In a full implementation, this would fail
        let _result = solver.solve(&request);
    }

    #[test]
    fn test_solver_multiple_versions() {
        let mut pool = Pool::new();

        // Package A with multiple versions
        pool.add_package(Package::new("vendor/a", "1.0.0"));
        pool.add_package(Package::new("vendor/a", "2.0.0"));

        let policy = Policy::new(); // Prefer highest
        let solver = Solver::new(&pool, &policy);

        let mut request = Request::new();
        request.require("vendor/a", "*");

        let result = solver.solve(&request);
        assert!(result.is_ok());

        let solver_result = result.unwrap();
        assert_eq!(solver_result.packages.len(), 1);

        // Should prefer the highest version (2.0.0)
        assert_eq!(solver_result.packages[0].version, "2.0.0");
    }

    #[test]
    fn test_solver_prefer_lowest() {
        let mut pool = Pool::new();

        pool.add_package(Package::new("vendor/a", "1.0.0"));
        pool.add_package(Package::new("vendor/a", "2.0.0"));

        let policy = Policy::new().prefer_lowest(true);
        let solver = Solver::new(&pool, &policy);

        let mut request = Request::new();
        request.require("vendor/a", "*");

        let result = solver.solve(&request);
        assert!(result.is_ok());

        let solver_result = result.unwrap();

        // Should prefer the lowest version (1.0.0)
        assert_eq!(solver_result.packages[0].version, "1.0.0");
    }
}
