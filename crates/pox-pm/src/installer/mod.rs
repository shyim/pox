//! Package installation system.
//!
//! This module handles the installation, update, and removal of packages
//! into the vendor directory.

mod binary;
mod library;
mod manager;
mod metapackage;
mod installer;

pub use binary::BinaryInstaller;
pub use library::LibraryInstaller;
pub use manager::{InstallConfig, InstallationManager};
pub use metapackage::{MetapackageInstaller, MetapackageResult};
pub use installer::Installer;
