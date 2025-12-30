// Package model for Composer packages
//
// This module provides structs and types for representing Composer packages,
// including dependencies, autoload configuration, source/dist information, etc.

mod alias;
mod autoload;
mod convert;
mod link;
mod package;
mod root_version;
mod source;

pub use alias::{
    parse_branch_aliases, parse_inline_alias, AliasPackage, DEFAULT_BRANCH_ALIAS,
};
pub use autoload::{Autoload, AutoloadPath};
pub use link::{Link, LinkType};
pub use package::{
    package_type, Abandoned, ArchiveConfig, Author, Funding, Package, ScriptHandler, Scripts,
    Stability, Support,
};
pub use root_version::{detect_root_version, get_git_branch, RootVersion, RootVersionSource};
pub use source::{Dist, Mirror, Source};
