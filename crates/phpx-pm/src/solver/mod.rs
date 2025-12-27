//! SAT-based dependency resolver for Composer packages.
//!
//! This module implements a CDCL (Conflict-Driven Clause Learning) SAT solver
//! specifically designed for package dependency resolution. The implementation
//! follows Composer's solver design.
//!
//! # Architecture
//!
//! The solver consists of several key components:
//!
//! - [`Pool`]: Registry of all available packages with lookup by name/constraint
//! - [`PoolOptimizer`]: Reduces pool size before solving for better performance
//! - [`Request`]: Specification of what needs to be resolved
//! - [`RuleSet`]: Collection of SAT clauses representing dependencies
//! - [`Solver`]: The main CDCL algorithm implementation
//!
//! # Algorithm Overview
//!
//! 1. **Pool Optimization** (optional, enabled by default): Reduce pool size by removing
//!    packages with identical dependencies and filtering impossible versions
//! 2. **Rule Generation**: Convert dependency graph to SAT clauses
//! 3. **Unit Propagation**: Force decisions from unit clauses
//! 4. **Decision Making**: Choose package versions using policy
//! 5. **Conflict Analysis**: Learn from conflicts to avoid repeating mistakes
//! 6. **Backtracking**: Revert to appropriate level on conflict
//!
//! # Example
//!
//! ```ignore
//! use phpx_pm::solver::{Pool, Request, Solver, Policy};
//!
//! let pool = Pool::new();
//! // ... add packages to pool
//!
//! let request = Request::new();
//! // ... add requirements to request
//!
//! let policy = Policy::default();
//! let solver = Solver::new(&pool, &policy);
//!
//! match solver.solve(&request) {
//!     Ok(transaction) => println!("Solution found!"),
//!     Err(problems) => println!("No solution: {:?}", problems),
//! }
//!
//! // To disable pool optimization:
//! let solver = Solver::new(&pool, &policy).with_optimization(false);
//! ```

mod pool;
mod pool_builder;
mod pool_optimizer;
mod request;
mod rule;
mod rule_set;
mod decisions;
mod watch_graph;
mod rule_generator;
mod solver;
mod problem;
mod transaction;
mod policy;

#[cfg(test)]
mod tests;

pub use pool::{Pool, PoolBuilder, PoolEntry, PackageId};
pub use pool_builder::PoolBuilder as LazyPoolBuilder;
pub use pool_optimizer::PoolOptimizer;
pub use request::Request;
pub use rule::{Rule, RuleType, Literal};
pub use rule_set::RuleSet;
pub use decisions::Decisions;
pub use solver::Solver;
pub use problem::Problem;
pub use transaction::{Transaction, Operation};
pub use policy::Policy;
