pub mod autoload;
pub mod cache;
pub mod composer;
pub mod config;
pub mod dependency_graph;
pub mod downloader;
pub mod error;
pub mod event;
pub mod http;
pub mod installer;
pub mod json;
pub mod package;
pub mod plugin;
pub mod repository;
pub mod scripts;
pub mod solver;
pub mod util;

pub use error::{ComposerError, Result};
pub use package::Package;
pub use json::{ComposerJson, ComposerLock};
pub use repository::{Repository, RepositoryManager};
pub use solver::{Pool, Request, Solver, Policy, Transaction};
pub use downloader::{DownloadManager, DownloadResult};
pub use installer::{InstallationManager, InstallConfig};
pub use autoload::{AutoloadGenerator, AutoloadConfig};
pub use plugin::{register_plugins, BinConfig};
pub use composer::{Composer, ComposerBuilder};
pub use dependency_graph::{get_dependents, find_packages_with_replacers_and_providers, DependencyResult};
pub use event::{
    ComposerEvent, EventDispatcher, EventListener, EventType,
    PostAutoloadDumpEvent, PostInstallEvent, PostUpdateEvent,
    PreAutoloadDumpEvent, PreInstallEvent, PreUpdateEvent,
};
pub use util::{is_platform_package, compute_content_hash};
#[cfg(test)] mod test_content_hash;
