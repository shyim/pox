//! VCS repository support - discovers packages from version control systems.
//!
//! This module provides repository implementations for:
//! - Generic VCS (auto-detect)
//! - Git repositories
//! - GitHub repositories (with API support)
//! - GitLab repositories (with API support)
//! - Bitbucket repositories (with API support)

mod driver;
mod git;
mod github;
mod gitlab;
mod bitbucket;
mod repository;

pub use driver::{VcsDriver, VcsDriverError, VcsInfo};
pub use git::{GitDriver, get_head_commit};
pub use github::GitHubDriver;
pub use gitlab::GitLabDriver;
pub use bitbucket::BitbucketDriver;
pub use repository::{VcsRepository, VcsType};
