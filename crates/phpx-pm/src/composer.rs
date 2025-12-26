use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, Result};

use crate::config::Config;
use crate::http::HttpClient;
use crate::json::{ComposerJson, ComposerLock};
use crate::repository::RepositoryManager;
use crate::installer::InstallationManager;
use crate::installer::InstallConfig;

/// The central Composer application object.
/// 
/// This struct holds the configuration and managers used throughout the application.
pub struct Composer {
    pub config: Config,
    pub composer_json: ComposerJson,
    pub composer_lock: Option<ComposerLock>,
    pub repository_manager: Arc<RepositoryManager>,
    pub installation_manager: Arc<InstallationManager>,
    pub http_client: Arc<HttpClient>,
    pub working_dir: PathBuf,
}

impl Composer {
    /// Create a new Composer instance
    pub fn new(
        working_dir: PathBuf,
        config: Config,
        composer_json: ComposerJson,
        composer_lock: Option<ComposerLock>,
    ) -> Result<Self> {
        let http_client = Arc::new(HttpClient::new().context("Failed to create HTTP client")?);
        
        // Initialize Repository Manager
        let repository_manager = Arc::new(RepositoryManager::new());
        
        
        let install_config = InstallConfig {
            vendor_dir: working_dir.join("vendor"), // Should come from config
            bin_dir: working_dir.join("vendor/bin"), // Should come from config
            cache_dir: config.cache_dir.clone().unwrap_or_else(|| PathBuf::from(".phpx/cache")), // Should leverage Config logic
            prefer_source: false, // Default, can be overridden
            prefer_dist: true,
            dry_run: false,
            no_dev: false,
        };

        let installation_manager = Arc::new(InstallationManager::new(
            http_client.clone(),
            install_config
        ));

        Ok(Self {
            config,
            composer_json,
            composer_lock,
            repository_manager,
            installation_manager,
            http_client,
            working_dir,
        })
    }
}
