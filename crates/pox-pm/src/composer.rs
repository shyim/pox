use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, Result};

use crate::config::{Config, PreferredInstall};
use crate::event::EventDispatcher;
use crate::http::HttpClient;
use crate::json::{ComposerJson, ComposerLock, Repository as JsonRepository, Repositories};
use crate::plugin::register_plugins;
use crate::repository::{ComposerRepository, RepositoryManager, Repository};
use crate::installer::InstallationManager;
use crate::installer::InstallConfig;

/// The central Composer application object.
pub struct Composer {
    pub config: Config,
    pub composer_json: ComposerJson,
    pub composer_lock: Option<ComposerLock>,
    pub repository_manager: Arc<RepositoryManager>,
    pub installation_manager: Arc<InstallationManager>,
    pub http_client: Arc<HttpClient>,
    pub working_dir: PathBuf,
    pub platform_packages: Vec<crate::package::Package>,
    pub event_dispatcher: EventDispatcher,
}

impl Composer {
    /// Create a new Composer instance using the builder pattern.
    pub fn builder(working_dir: PathBuf) -> ComposerBuilder {
        ComposerBuilder::new(working_dir)
    }

    /// Create a new Composer instance directly.
    pub fn new(
        working_dir: PathBuf,
        config: Config,
        composer_json: ComposerJson,
        composer_lock: Option<ComposerLock>,
    ) -> Result<Self> {
        ComposerBuilder::new(working_dir)
            .with_config(config)
            .with_composer_json(composer_json)
            .with_composer_lock(composer_lock)
            .build()
    }

    /// Dispatch a typed event and return the exit code.
    pub fn dispatch<E: crate::event::ComposerEvent>(&self, event: &E) -> anyhow::Result<i32> {
        self.event_dispatcher.dispatch(event, self)
    }

    /// Get the vendor directory path.
    pub fn vendor_dir(&self) -> std::path::PathBuf {
        self.working_dir.join(&self.config.vendor_dir)
    }
}

/// Builder for creating Composer instances.
pub struct ComposerBuilder {
    working_dir: PathBuf,
    config: Option<Config>,
    composer_json: Option<ComposerJson>,
    composer_lock: Option<ComposerLock>,
    http_client: Option<Arc<HttpClient>>,
    repository_manager: Option<RepositoryManager>,
    additional_repositories: Vec<Arc<dyn Repository>>,

    // Installation options (override config)
    prefer_source: Option<bool>,
    prefer_dist: Option<bool>,
    dry_run: bool,
    no_dev: bool,
    prefer_lowest: bool,

    // Platform packages (php, ext-*, lib-*)
    platform_packages: Vec<crate::package::Package>,

    // Repository options
    disable_packagist: Option<bool>,
}

impl ComposerBuilder {
    /// Create a new builder with the given working directory.
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir,
            config: None,
            composer_json: None,
            composer_lock: None,
            http_client: None,
            repository_manager: None,
            additional_repositories: Vec::new(),
            prefer_source: None,
            prefer_dist: None,
            dry_run: false,
            no_dev: false,
            prefer_lowest: false,
            platform_packages: Vec::new(),
            disable_packagist: None,
        }
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_composer_json(mut self, composer_json: ComposerJson) -> Self {
        self.composer_json = Some(composer_json);
        self
    }

    pub fn with_composer_lock(mut self, composer_lock: Option<ComposerLock>) -> Self {
        self.composer_lock = composer_lock;
        self
    }

    pub fn with_http_client(mut self, http_client: Arc<HttpClient>) -> Self {
        self.http_client = Some(http_client);
        self
    }

    pub fn with_repository_manager(mut self, repository_manager: RepositoryManager) -> Self {
        self.repository_manager = Some(repository_manager);
        self
    }

    pub fn add_repository(mut self, repo: Arc<dyn Repository>) -> Self {
        self.additional_repositories.push(repo);
        self
    }

    pub fn prefer_source(mut self, prefer: bool) -> Self {
        self.prefer_source = Some(prefer);
        if prefer {
            self.prefer_dist = Some(false);
        }
        self
    }

    pub fn prefer_dist(mut self, prefer: bool) -> Self {
        self.prefer_dist = Some(prefer);
        if prefer {
            self.prefer_source = Some(false);
        }
        self
    }

    pub fn dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    pub fn no_dev(mut self, no_dev: bool) -> Self {
        self.no_dev = no_dev;
        self
    }

    pub fn prefer_lowest(mut self, prefer: bool) -> Self {
        self.prefer_lowest = prefer;
        self
    }

    pub fn with_platform_packages(mut self, packages: Vec<crate::package::Package>) -> Self {
        self.platform_packages = packages;
        self
    }

    pub fn disable_packagist(mut self, disable: bool) -> Self {
        self.disable_packagist = Some(disable);
        self
    }

    pub fn build(mut self) -> Result<Composer> {
        let composer_json = self.composer_json.take()
            .ok_or_else(|| anyhow::anyhow!("composer.json is required"))?;

        let config = self.config.take().unwrap_or_else(|| {
            Config::with_base_dir(&self.working_dir)
        });

        let http_client = match self.http_client.take() {
            Some(client) => client,
            None => Arc::new(HttpClient::new().context("Failed to create HTTP client")?),
        };

        let repository_manager = self.build_repository_manager(&config, &composer_json)?;
        let install_config = self.build_install_config(&config);

        let installation_manager = Arc::new(InstallationManager::new(
            http_client.clone(),
            install_config,
        ));

        // Create event dispatcher with script listeners and plugins
        let mut event_dispatcher = EventDispatcher::with_scripts();
        register_plugins(&mut event_dispatcher);

        Ok(Composer {
            config,
            composer_json,
            composer_lock: self.composer_lock.take(),
            repository_manager: Arc::new(repository_manager),
            installation_manager,
            http_client,
            working_dir: self.working_dir.clone(),
            platform_packages: std::mem::take(&mut self.platform_packages),
            event_dispatcher,
        })
    }

    fn build_repository_manager(
        &mut self,
        config: &Config,
        composer_json: &ComposerJson,
    ) -> Result<RepositoryManager> {
        if let Some(manager) = self.repository_manager.take() {
            return Ok(manager);
        }

        let mut repository_manager = RepositoryManager::new();

        for repo in composer_json.repositories.as_vec() {
            repository_manager.add_from_json_repository(&repo);
        }

        for repo in &self.additional_repositories {
            repository_manager.add_repository(repo.clone());
        }

        let packagist_disabled = self.disable_packagist.unwrap_or_else(|| {
            is_packagist_disabled(&composer_json.repositories)
        });

        if !packagist_disabled {
            let packagist = if let Some(cache_dir) = config.cache_dir.clone() {
                ComposerRepository::packagist_with_cache(cache_dir)
            } else {
                ComposerRepository::packagist()
            };
            repository_manager.add_repository(Arc::new(packagist));
        }

        Ok(repository_manager)
    }

    fn build_install_config(&self, config: &Config) -> InstallConfig {
        let (prefer_source, prefer_dist) = match (self.prefer_source, self.prefer_dist) {
            (Some(src), Some(dst)) => (src, dst),
            (Some(src), None) => (src, !src),
            (None, Some(dst)) => (!dst, dst),
            (None, None) => {
                match config.preferred_install {
                    PreferredInstall::Source => (true, false),
                    PreferredInstall::Dist => (false, true),
                    PreferredInstall::Auto => (false, true), // Default to dist
                }
            }
        };

        InstallConfig {
            vendor_dir: self.working_dir.join(&config.vendor_dir),
            bin_dir: self.working_dir.join(&config.bin_dir),
            cache_dir: config.cache_dir.clone()
                .unwrap_or_else(|| self.working_dir.join(".pox/cache")),
            prefer_source,
            prefer_dist,
            dry_run: self.dry_run,
            no_dev: self.no_dev,
            prefer_lowest: self.prefer_lowest,
        }
    }
}

impl Clone for ComposerBuilder {
    fn clone(&self) -> Self {
        Self {
            working_dir: self.working_dir.clone(),
            config: self.config.clone(),
            composer_json: self.composer_json.clone(),
            composer_lock: self.composer_lock.clone(),
            http_client: self.http_client.clone(),
            repository_manager: None, // RepositoryManager doesn't implement Clone
            additional_repositories: self.additional_repositories.clone(),
            prefer_source: self.prefer_source,
            prefer_dist: self.prefer_dist,
            dry_run: self.dry_run,
            no_dev: self.no_dev,
            prefer_lowest: self.prefer_lowest,
            platform_packages: self.platform_packages.clone(),
            disable_packagist: self.disable_packagist,
        }
    }
}

/// Check if packagist.org is disabled in the repositories configuration
fn is_packagist_disabled(repositories: &Repositories) -> bool {
    match repositories {
        Repositories::None => false,
        Repositories::Array(repos) => {
            // In array format, check for Disabled(false) entries
            // (though this is unusual - disabling is typically done in object format)
            repos.iter().any(|r| matches!(r, JsonRepository::Disabled(false)))
        }
        Repositories::Object(map) => {
            // In object format, packagist.org is disabled if key exists with false value
            map.iter().any(|(key, val)| {
                (key == "packagist.org" || key == "packagist")
                    && matches!(val, JsonRepository::Disabled(false))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    fn create_minimal_composer_json() -> ComposerJson {
        ComposerJson {
            name: Some("test/package".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_builder_minimal() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();

        let result = ComposerBuilder::new(working_dir.clone())
            .with_composer_json(composer_json)
            .build();

        assert!(result.is_ok());
        let composer = result.unwrap();
        assert_eq!(composer.working_dir, working_dir);
    }

    #[test]
    fn test_builder_missing_composer_json() {
        let working_dir = PathBuf::from("/tmp/test");

        let result = ComposerBuilder::new(working_dir).build();

        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("composer.json is required"));
    }

    #[test]
    fn test_builder_with_dry_run() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();

        let _composer = ComposerBuilder::new(working_dir)
            .with_composer_json(composer_json)
            .dry_run(true)
            .build()
            .unwrap();

    }

    #[test]
    fn test_builder_with_no_dev() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();

        let _composer = ComposerBuilder::new(working_dir)
            .with_composer_json(composer_json)
            .no_dev(true)
            .build()
            .unwrap();
    }

    #[test]
    fn test_builder_prefer_source() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();

        let _composer = ComposerBuilder::new(working_dir)
            .with_composer_json(composer_json)
            .prefer_source(true)
            .build()
            .unwrap();
    }

    #[test]
    fn test_builder_prefer_dist() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();

        let _composer = ComposerBuilder::new(working_dir)
            .with_composer_json(composer_json)
            .prefer_dist(true)
            .build()
            .unwrap();
    }

    #[test]
    fn test_builder_disable_packagist() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();

        let composer = ComposerBuilder::new(working_dir)
            .with_composer_json(composer_json)
            .disable_packagist(true)
            .build()
            .unwrap();

        let repos = composer.repository_manager.repositories();
        let has_packagist = repos.iter().any(|r| r.name().contains("packagist"));
        assert!(!has_packagist);
    }

    #[test]
    fn test_builder_with_config() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();
        let config = Config::with_base_dir(&working_dir);

        let composer = ComposerBuilder::new(working_dir.clone())
            .with_config(config)
            .with_composer_json(composer_json)
            .build()
            .unwrap();

        assert_eq!(composer.config.base_dir(), Some(working_dir.as_path()));
    }

    #[test]
    fn test_builder_with_lock() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();
        let composer_lock = ComposerLock::default();

        let composer = ComposerBuilder::new(working_dir)
            .with_composer_json(composer_json)
            .with_composer_lock(Some(composer_lock))
            .build()
            .unwrap();

        assert!(composer.composer_lock.is_some());
    }

    #[test]
    fn test_builder_clone() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();

        let builder = ComposerBuilder::new(working_dir)
            .with_composer_json(composer_json)
            .dry_run(true)
            .no_dev(true);

        let cloned = builder.clone();
        assert_eq!(cloned.dry_run, true);
        assert_eq!(cloned.no_dev, true);
    }

    #[test]
    fn test_composer_builder_static_method() {
        let working_dir = PathBuf::from("/tmp/test");
        let composer_json = create_minimal_composer_json();

        let composer = Composer::builder(working_dir.clone())
            .with_composer_json(composer_json)
            .build()
            .unwrap();

        assert_eq!(composer.working_dir, working_dir);
    }

    #[test]
    fn test_is_packagist_disabled_none() {
        let repos = Repositories::None;
        assert!(!is_packagist_disabled(&repos));
    }

    #[test]
    fn test_is_packagist_disabled_empty_array() {
        let repos = Repositories::Array(vec![]);
        assert!(!is_packagist_disabled(&repos));
    }

    #[test]
    fn test_is_packagist_disabled_array_with_disabled() {
        let repos = Repositories::Array(vec![JsonRepository::Disabled(false)]);
        assert!(is_packagist_disabled(&repos));
    }

    #[test]
    fn test_is_packagist_disabled_empty_object() {
        let repos = Repositories::Object(IndexMap::new());
        assert!(!is_packagist_disabled(&repos));
    }

    #[test]
    fn test_is_packagist_disabled_object_packagist_org_false() {
        let mut map = IndexMap::new();
        map.insert("packagist.org".to_string(), JsonRepository::Disabled(false));
        let repos = Repositories::Object(map);
        assert!(is_packagist_disabled(&repos));
    }

    #[test]
    fn test_is_packagist_disabled_object_packagist_false() {
        let mut map = IndexMap::new();
        map.insert("packagist".to_string(), JsonRepository::Disabled(false));
        let repos = Repositories::Object(map);
        assert!(is_packagist_disabled(&repos));
    }

    #[test]
    fn test_is_packagist_disabled_object_other_repo() {
        let mut map = IndexMap::new();
        map.insert("other-repo".to_string(), JsonRepository::Disabled(false));
        let repos = Repositories::Object(map);
        assert!(!is_packagist_disabled(&repos));
    }
}
