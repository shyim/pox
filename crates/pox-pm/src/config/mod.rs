//! Configuration management for Composer
//!
//! This module provides comprehensive configuration handling that matches Composer's behavior,
//! including loading from multiple sources (defaults, global config, project config, environment
//! variables) and merging them in the correct priority order.
//!
//! # Configuration Sources (in priority order, highest to lowest)
//!
//! 1. Environment variables (`COMPOSER_*`)
//! 2. Project `composer.json` config section
//! 3. Global `~/.composer/config.json`
//! 4. Built-in defaults
//!
//! # Authentication
//!
//! Authentication credentials are loaded from `auth.json` files:
//! - Global: `~/.composer/auth.json`
//! - Project: `./auth.json`
//! - Environment: `COMPOSER_AUTH` (JSON string)
//!
//! # Example
//!
//! ```rust,no_run
//! use pox_pm::config::{Config, AuthConfig};
//! use std::path::Path;
//!
//! // Build configuration for a project
//! let config = Config::build(Some(Path::new("/path/to/project")), true).unwrap();
//!
//! // Load authentication
//! let auth = AuthConfig::build(Some(Path::new("/path/to/project"))).unwrap();
//!
//! // Access configuration values
//! println!("Vendor dir: {:?}", config.get_vendor_dir());
//! println!("Process timeout: {}", config.process_timeout);
//!
//! // Check for GitHub token
//! if let Some(token) = auth.get_github_oauth("github.com") {
//!     println!("GitHub token: {}", token);
//! }
//! ```

mod auth;
mod config;
mod source;

pub use auth::{AuthConfig, AuthMatch, BitbucketOAuthCredentials, GitLabAuth, HttpBasicCredentials};
pub use config::{
    AllowPlugins, AuditConfig, BitbucketOAuth, Config, DiscardChanges, GitLabToken,
    HttpBasicAuth, PlatformCheck, PreferredInstall, StoreAuths,
};
pub use source::{ConfigLoader, ConfigSource, RawConfig};
