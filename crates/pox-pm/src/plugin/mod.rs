//! Plugin system for ported Composer plugins.
//!
//! This module provides native Rust implementations of popular Composer plugins.
//! Since pox cannot execute PHP-based Composer plugins, these are manually
//! ported and registered as event listeners.
//!
//! Each plugin implements `EventListener` directly and checks if its
//! corresponding package is installed before taking action.

mod composer_bin;
mod phpstan_extension_installer;
mod registry;
mod symfony_runtime;

pub use composer_bin::BinConfig;
pub use registry::register_plugins;
