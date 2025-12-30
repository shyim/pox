use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::Result;
use super::source::{ConfigLoader, ConfigSource, RawConfig};

/// Preferred installation method
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreferredInstall {
    Auto,
    Source,
    Dist,
}

impl Default for PreferredInstall {
    fn default() -> Self {
        PreferredInstall::Dist
    }
}

impl PreferredInstall {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "auto" => Some(PreferredInstall::Auto),
            "source" => Some(PreferredInstall::Source),
            "dist" => Some(PreferredInstall::Dist),
            _ => None,
        }
    }
}

/// How to handle authentication storage
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreAuths {
    #[serde(rename = "true")]
    True,
    #[serde(rename = "false")]
    False,
    Prompt,
}

impl Default for StoreAuths {
    fn default() -> Self {
        StoreAuths::Prompt
    }
}

impl StoreAuths {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "true" => Some(StoreAuths::True),
            "false" => Some(StoreAuths::False),
            "prompt" => Some(StoreAuths::Prompt),
            _ => None,
        }
    }
}

/// How to handle uncommitted changes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscardChanges {
    #[serde(rename = "true")]
    True,
    #[serde(rename = "false")]
    False,
    Stash,
}

impl Default for DiscardChanges {
    fn default() -> Self {
        DiscardChanges::False
    }
}

impl DiscardChanges {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "true" => Some(DiscardChanges::True),
            "false" => Some(DiscardChanges::False),
            "stash" => Some(DiscardChanges::Stash),
            _ => None,
        }
    }
}

/// Platform check configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlatformCheck {
    PhpOnly,
    True,
    False,
}

impl Default for PlatformCheck {
    fn default() -> Self {
        PlatformCheck::PhpOnly
    }
}

impl PlatformCheck {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "php-only" => Some(PlatformCheck::PhpOnly),
            "true" => Some(PlatformCheck::True),
            "false" => Some(PlatformCheck::False),
            _ => None,
        }
    }
}

/// Plugin allowlist configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AllowPlugins {
    Bool(bool),
    Map(HashMap<String, bool>),
}

impl Default for AllowPlugins {
    fn default() -> Self {
        AllowPlugins::Map(HashMap::new())
    }
}

/// Audit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    #[serde(default)]
    pub ignore: Vec<String>,

    #[serde(default = "default_audit_abandoned")]
    pub abandoned: String,
}

fn default_audit_abandoned() -> String {
    "fail".to_string()
}

impl Default for AuditConfig {
    fn default() -> Self {
        AuditConfig {
            ignore: Vec::new(),
            abandoned: default_audit_abandoned(),
        }
    }
}

/// HTTP Basic authentication credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpBasicAuth {
    pub username: String,
    pub password: String,
}

/// GitLab token authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GitLabToken {
    Token(String),
    OAuth {
        #[serde(rename = "oauth-token")]
        oauth_token: String,
    },
}

/// Bitbucket OAuth configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitbucketOAuth {
    #[serde(rename = "consumer-key")]
    pub consumer_key: String,

    #[serde(rename = "consumer-secret")]
    pub consumer_secret: String,
}

/// Main Composer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // Directories
    #[serde(rename = "vendor-dir", default = "default_vendor_dir")]
    pub vendor_dir: PathBuf,

    #[serde(rename = "bin-dir", default = "default_bin_dir")]
    pub bin_dir: PathBuf,

    #[serde(rename = "cache-dir", skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,

    #[serde(rename = "data-dir", skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,

    #[serde(rename = "home", skip_serializing_if = "Option::is_none")]
    pub home_dir: Option<PathBuf>,

    // Cache settings
    #[serde(rename = "cache-files-dir", skip_serializing_if = "Option::is_none")]
    pub cache_files_dir: Option<PathBuf>,

    #[serde(rename = "cache-repo-dir", skip_serializing_if = "Option::is_none")]
    pub cache_repo_dir: Option<PathBuf>,

    #[serde(rename = "cache-vcs-dir", skip_serializing_if = "Option::is_none")]
    pub cache_vcs_dir: Option<PathBuf>,

    #[serde(rename = "cache-ttl", default = "default_cache_ttl")]
    pub cache_ttl: u64,

    #[serde(rename = "cache-files-ttl", skip_serializing_if = "Option::is_none")]
    pub cache_files_ttl: Option<u64>,

    #[serde(rename = "cache-files-maxsize", default = "default_cache_files_maxsize")]
    pub cache_files_maxsize: u64,

    #[serde(rename = "cache-read-only", default)]
    pub cache_read_only: bool,

    // Behavior
    #[serde(rename = "process-timeout", default = "default_process_timeout")]
    pub process_timeout: u64,

    #[serde(rename = "use-include-path", default)]
    pub use_include_path: bool,

    #[serde(rename = "use-parent-dir", skip_serializing_if = "Option::is_none")]
    pub use_parent_dir: Option<String>,

    #[serde(rename = "preferred-install", default)]
    pub preferred_install: PreferredInstall,

    #[serde(rename = "store-auths", default)]
    pub store_auths: StoreAuths,

    #[serde(rename = "notify-on-install", default = "default_true")]
    pub notify_on_install: bool,

    #[serde(rename = "discard-changes", default)]
    pub discard_changes: DiscardChanges,

    #[serde(rename = "optimize-autoloader", default)]
    pub optimize_autoloader: bool,

    #[serde(rename = "sort-packages", default)]
    pub sort_packages: bool,

    #[serde(rename = "classmap-authoritative", default)]
    pub classmap_authoritative: bool,

    #[serde(rename = "apcu-autoloader", default)]
    pub apcu_autoloader: bool,

    #[serde(rename = "prepend-autoloader", default = "default_true")]
    pub prepend_autoloader: bool,

    #[serde(rename = "autoloader-suffix", skip_serializing_if = "Option::is_none")]
    pub autoloader_suffix: Option<String>,

    #[serde(rename = "lock", default = "default_true")]
    pub lock: bool,

    #[serde(rename = "platform-check", default)]
    pub platform_check: PlatformCheck,

    #[serde(rename = "allow-plugins", default)]
    pub allow_plugins: AllowPlugins,

    #[serde(default)]
    pub audit: AuditConfig,

    // Network - Security
    #[serde(rename = "secure-http", default = "default_true")]
    pub secure_http: bool,

    #[serde(rename = "disable-tls", default)]
    pub disable_tls: bool,

    #[serde(rename = "secure-svn-domains", default)]
    pub secure_svn_domains: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cafile: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub capath: Option<PathBuf>,

    // Network - Protocols
    #[serde(rename = "github-protocols", default = "default_github_protocols")]
    pub github_protocols: Vec<String>,

    #[serde(rename = "gitlab-protocol", skip_serializing_if = "Option::is_none")]
    pub gitlab_protocol: Option<String>,

    // Network - Domains
    #[serde(rename = "github-domains", default = "default_github_domains")]
    pub github_domains: Vec<String>,

    #[serde(rename = "gitlab-domains", default = "default_gitlab_domains")]
    pub gitlab_domains: Vec<String>,

    #[serde(rename = "bitbucket-domains", default)]
    pub bitbucket_domains: Vec<String>,

    #[serde(rename = "forgejo-domains", default = "default_forgejo_domains")]
    pub forgejo_domains: Vec<String>,

    // Network - API settings
    #[serde(rename = "use-github-api", default = "default_true")]
    pub use_github_api: bool,

    #[serde(rename = "github-expose-hostname", default = "default_true")]
    pub github_expose_hostname: bool,

    #[serde(rename = "bitbucket-expose-hostname", default = "default_true")]
    pub bitbucket_expose_hostname: bool,

    // Authentication
    #[serde(rename = "http-basic", default)]
    pub http_basic: HashMap<String, HttpBasicAuth>,

    #[serde(default)]
    pub bearer: HashMap<String, String>,

    #[serde(rename = "github-oauth", default)]
    pub github_oauth: HashMap<String, String>,

    #[serde(rename = "gitlab-oauth", default)]
    pub gitlab_oauth: HashMap<String, String>,

    #[serde(rename = "gitlab-token", default)]
    pub gitlab_token: HashMap<String, GitLabToken>,

    #[serde(rename = "bitbucket-oauth", default)]
    pub bitbucket_oauth: HashMap<String, BitbucketOAuth>,

    #[serde(rename = "forgejo-token", default)]
    pub forgejo_token: HashMap<String, String>,

    // Platform overrides
    #[serde(default)]
    pub platform: HashMap<String, String>,

    // Archive settings
    #[serde(rename = "archive-format", default = "default_archive_format")]
    pub archive_format: String,

    #[serde(rename = "archive-dir", default = "default_archive_dir")]
    pub archive_dir: PathBuf,

    // Misc
    #[serde(rename = "htaccess-protect", default = "default_true")]
    pub htaccess_protect: bool,

    #[serde(rename = "bin-compat", default = "default_bin_compat")]
    pub bin_compat: String,

    #[serde(rename = "custom-headers", default)]
    pub custom_headers: HashMap<String, String>,

    #[serde(rename = "client-certificate", default)]
    pub client_certificate: HashMap<String, serde_json::Value>,

    // Internal tracking
    #[serde(skip)]
    base_dir: Option<PathBuf>,

    #[serde(skip)]
    sources: HashMap<String, ConfigSource>,
}

// Default value functions
fn default_vendor_dir() -> PathBuf {
    PathBuf::from("vendor")
}

fn default_bin_dir() -> PathBuf {
    PathBuf::from("vendor/bin")
}

fn default_process_timeout() -> u64 {
    300
}

fn default_cache_ttl() -> u64 {
    15552000 // 6 months in seconds
}

fn default_cache_files_maxsize() -> u64 {
    300 * 1024 * 1024 // 300 MiB
}

fn default_github_protocols() -> Vec<String> {
    vec!["https".to_string(), "ssh".to_string(), "git".to_string()]
}

fn default_github_domains() -> Vec<String> {
    vec!["github.com".to_string()]
}

fn default_gitlab_domains() -> Vec<String> {
    vec!["gitlab.com".to_string()]
}

fn default_forgejo_domains() -> Vec<String> {
    vec!["codeberg.org".to_string()]
}

fn default_archive_format() -> String {
    "tar".to_string()
}

fn default_archive_dir() -> PathBuf {
    PathBuf::from(".")
}

fn default_bin_compat() -> String {
    "auto".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // Directories
            vendor_dir: default_vendor_dir(),
            bin_dir: default_bin_dir(),
            cache_dir: None,
            data_dir: None,
            home_dir: None,
            cache_files_dir: None,
            cache_repo_dir: None,
            cache_vcs_dir: None,
            cache_ttl: default_cache_ttl(),
            cache_files_ttl: None,
            cache_files_maxsize: default_cache_files_maxsize(),
            cache_read_only: false,

            // Behavior
            process_timeout: default_process_timeout(),
            use_include_path: false,
            use_parent_dir: Some("prompt".to_string()),
            preferred_install: PreferredInstall::default(),
            store_auths: StoreAuths::default(),
            notify_on_install: true,
            discard_changes: DiscardChanges::default(),
            optimize_autoloader: false,
            sort_packages: false,
            classmap_authoritative: false,
            apcu_autoloader: false,
            prepend_autoloader: true,
            autoloader_suffix: None,
            lock: true,
            platform_check: PlatformCheck::default(),
            allow_plugins: AllowPlugins::default(),
            audit: AuditConfig::default(),

            // Network - Security
            secure_http: true,
            disable_tls: false,
            secure_svn_domains: Vec::new(),
            cafile: None,
            capath: None,

            // Network - Protocols
            github_protocols: default_github_protocols(),
            gitlab_protocol: None,

            // Network - Domains
            github_domains: default_github_domains(),
            gitlab_domains: default_gitlab_domains(),
            bitbucket_domains: Vec::new(),
            forgejo_domains: default_forgejo_domains(),

            // Network - API settings
            use_github_api: true,
            github_expose_hostname: true,
            bitbucket_expose_hostname: true,

            // Authentication
            http_basic: HashMap::new(),
            bearer: HashMap::new(),
            github_oauth: HashMap::new(),
            gitlab_oauth: HashMap::new(),
            gitlab_token: HashMap::new(),
            bitbucket_oauth: HashMap::new(),
            forgejo_token: HashMap::new(),

            // Platform overrides
            platform: HashMap::new(),

            // Archive settings
            archive_format: default_archive_format(),
            archive_dir: default_archive_dir(),

            // Misc
            htaccess_protect: true,
            bin_compat: default_bin_compat(),
            custom_headers: HashMap::new(),
            client_certificate: HashMap::new(),

            // Internal
            base_dir: None,
            sources: HashMap::new(),
        }
    }
}

impl Config {
    /// Create a new Config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new Config with defaults and base directory
    pub fn with_base_dir<P: AsRef<Path>>(base_dir: P) -> Self {
        let mut config = Self::default();
        config.base_dir = Some(base_dir.as_ref().to_path_buf());
        config
    }

    /// Build configuration from all sources (defaults, global, project, env)
    pub fn build<P: AsRef<Path>>(project_dir: Option<P>, use_environment: bool) -> Result<Self> {
        let loader = ConfigLoader::new(use_environment);
        let mut config = Self::default();

        if let Some(ref dir) = project_dir {
            config.base_dir = Some(dir.as_ref().to_path_buf());
        }

        for key in config.config_keys() {
            config.sources.insert(key, ConfigSource::Default);
        }

        // 1. Load global config from ~/.composer/config.json
        let global_config = loader.load_global_config()?;
        config.merge_raw_config(global_config, ConfigSource::Global)?;

        // 2. Load project config from composer.json
        if let Some(project_dir) = &project_dir {
            let project_config = loader.load_project_config(project_dir)?;
            config.merge_raw_config(project_config, ConfigSource::Project)?;
        }

        // 3. Apply environment variable overrides
        if use_environment {
            config.apply_env_overrides(&loader);
        }

        // 4. Resolve computed paths
        config.resolve_paths(&loader);

        Ok(config)
    }

    /// Set base directory (must be absolute path)
    pub fn set_base_dir<P: AsRef<Path>>(&mut self, base_dir: P) {
        self.base_dir = Some(base_dir.as_ref().to_path_buf());
    }

    /// Get base directory
    pub fn base_dir(&self) -> Option<&Path> {
        self.base_dir.as_deref()
    }

    /// Get the source of a configuration value
    pub fn get_source(&self, key: &str) -> Option<&ConfigSource> {
        self.sources.get(key)
    }

    /// Get vendor directory (resolved as absolute path)
    pub fn get_vendor_dir(&self) -> PathBuf {
        self.resolve_path(&self.vendor_dir)
    }

    /// Get bin directory (resolved as absolute path)
    pub fn get_bin_dir(&self) -> PathBuf {
        self.resolve_path(&self.bin_dir)
    }

    /// Get cache directory (resolved as absolute path)
    pub fn get_cache_dir(&self, loader: &ConfigLoader) -> PathBuf {
        if let Some(ref cache_dir) = self.cache_dir {
            self.resolve_path(cache_dir)
        } else {
            loader.get_cache_dir()
        }
    }

    /// Get data directory (resolved as absolute path)
    pub fn get_data_dir(&self, loader: &ConfigLoader) -> PathBuf {
        if let Some(ref data_dir) = self.data_dir {
            self.resolve_path(data_dir)
        } else {
            loader.get_composer_home()
        }
    }

    /// Resolve a path relative to base_dir if not absolute
    fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(ref base) = self.base_dir {
            base.join(path)
        } else {
            path.to_path_buf()
        }
    }

    /// Merge raw configuration from a source
    fn merge_raw_config(&mut self, raw: RawConfig, source: ConfigSource) -> Result<()> {
        if let Some(config_map) = raw.config {
            for (key, value) in config_map {
                self.merge_config_value(&key, value, source.clone())?;
            }
        }
        Ok(())
    }

    /// Merge a single configuration value
    fn merge_config_value(
        &mut self,
        key: &str,
        value: serde_json::Value,
        source: ConfigSource,
    ) -> Result<()> {
        match key {
            "vendor-dir" => {
                if let Some(s) = value.as_str() {
                    self.vendor_dir = PathBuf::from(s);
                    self.sources.insert(key.to_string(), source);
                }
            }
            "bin-dir" => {
                if let Some(s) = value.as_str() {
                    self.bin_dir = PathBuf::from(s);
                    self.sources.insert(key.to_string(), source);
                }
            }
            "cache-dir" => {
                if let Some(s) = value.as_str() {
                    self.cache_dir = Some(PathBuf::from(s));
                    self.sources.insert(key.to_string(), source);
                }
            }
            "data-dir" => {
                if let Some(s) = value.as_str() {
                    self.data_dir = Some(PathBuf::from(s));
                    self.sources.insert(key.to_string(), source);
                }
            }
            "process-timeout" => {
                if let Some(n) = value.as_u64() {
                    self.process_timeout = n;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "use-include-path" => {
                if let Some(b) = value.as_bool() {
                    self.use_include_path = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "preferred-install" => {
                if let Some(s) = value.as_str() {
                    if let Some(pi) = PreferredInstall::from_str(s) {
                        self.preferred_install = pi;
                        self.sources.insert(key.to_string(), source);
                    }
                }
            }
            "store-auths" => {
                if let Some(s) = value.as_str() {
                    if let Some(sa) = StoreAuths::from_str(s) {
                        self.store_auths = sa;
                        self.sources.insert(key.to_string(), source);
                    }
                }
            }
            "notify-on-install" => {
                if let Some(b) = value.as_bool() {
                    self.notify_on_install = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "discard-changes" => {
                if let Some(s) = value.as_str() {
                    if let Some(dc) = DiscardChanges::from_str(s) {
                        self.discard_changes = dc;
                        self.sources.insert(key.to_string(), source);
                    }
                } else if let Some(b) = value.as_bool() {
                    self.discard_changes = if b { DiscardChanges::True } else { DiscardChanges::False };
                    self.sources.insert(key.to_string(), source);
                }
            }
            "optimize-autoloader" => {
                if let Some(b) = value.as_bool() {
                    self.optimize_autoloader = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "sort-packages" => {
                if let Some(b) = value.as_bool() {
                    self.sort_packages = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "classmap-authoritative" => {
                if let Some(b) = value.as_bool() {
                    self.classmap_authoritative = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "apcu-autoloader" => {
                if let Some(b) = value.as_bool() {
                    self.apcu_autoloader = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "secure-http" => {
                if let Some(b) = value.as_bool() {
                    self.secure_http = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "disable-tls" => {
                if let Some(b) = value.as_bool() {
                    self.disable_tls = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "lock" => {
                if let Some(b) = value.as_bool() {
                    self.lock = b;
                    self.sources.insert(key.to_string(), source);
                }
            }
            "platform-check" => {
                if let Some(s) = value.as_str() {
                    if let Some(pc) = PlatformCheck::from_str(s) {
                        self.platform_check = pc;
                        self.sources.insert(key.to_string(), source);
                    }
                } else if let Some(b) = value.as_bool() {
                    self.platform_check = if b { PlatformCheck::True } else { PlatformCheck::False };
                    self.sources.insert(key.to_string(), source);
                }
            }
            "github-protocols" => {
                if let Some(arr) = value.as_array() {
                    self.github_protocols = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    self.sources.insert(key.to_string(), source);
                }
            }
            "github-domains" => {
                if let Some(arr) = value.as_array() {
                    let new_domains: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    // Merge with existing
                    for domain in new_domains {
                        if !self.github_domains.contains(&domain) {
                            self.github_domains.push(domain);
                        }
                    }
                    self.sources.insert(key.to_string(), source);
                }
            }
            "gitlab-domains" => {
                if let Some(arr) = value.as_array() {
                    let new_domains: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    // Merge with existing
                    for domain in new_domains {
                        if !self.gitlab_domains.contains(&domain) {
                            self.gitlab_domains.push(domain);
                        }
                    }
                    self.sources.insert(key.to_string(), source);
                }
            }
            "platform" => {
                if let Some(obj) = value.as_object() {
                    for (k, v) in obj {
                        if let Some(s) = v.as_str() {
                            self.platform.insert(k.clone(), s.to_string());
                        }
                    }
                    self.sources.insert(key.to_string(), source);
                }
            }
            "github-oauth" => {
                if let Some(obj) = value.as_object() {
                    for (k, v) in obj {
                        if let Some(s) = v.as_str() {
                            self.github_oauth.insert(k.clone(), s.to_string());
                        }
                    }
                    self.sources.insert(key.to_string(), source);
                }
            }
            "gitlab-oauth" => {
                if let Some(obj) = value.as_object() {
                    for (k, v) in obj {
                        if let Some(s) = v.as_str() {
                            self.gitlab_oauth.insert(k.clone(), s.to_string());
                        }
                    }
                    self.sources.insert(key.to_string(), source);
                }
            }
            _ => {
                // For unknown keys, store the source but don't fail
                self.sources.insert(key.to_string(), source);
            }
        }

        Ok(())
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(&mut self, loader: &ConfigLoader) {
        // Process timeout
        if let Some(timeout) = loader.get_env_u64("process-timeout") {
            self.process_timeout = timeout;
            self.sources.insert(
                "process-timeout".to_string(),
                ConfigSource::Environment("COMPOSER_PROCESS_TIMEOUT".to_string()),
            );
        }

        // Cache directory
        if let Some(cache_dir) = loader.get_env_path("cache-dir") {
            self.cache_dir = Some(cache_dir);
            self.sources.insert(
                "cache-dir".to_string(),
                ConfigSource::Environment("COMPOSER_CACHE_DIR".to_string()),
            );
        }

        // Vendor directory
        if let Some(vendor_dir) = loader.get_env_path("vendor-dir") {
            self.vendor_dir = vendor_dir;
            self.sources.insert(
                "vendor-dir".to_string(),
                ConfigSource::Environment("COMPOSER_VENDOR_DIR".to_string()),
            );
        }

        // Bin directory
        if let Some(bin_dir) = loader.get_env_path("bin-dir") {
            self.bin_dir = bin_dir;
            self.sources.insert(
                "bin-dir".to_string(),
                ConfigSource::Environment("COMPOSER_BIN_DIR".to_string()),
            );
        }

        // Discard changes
        if let Some(discard) = loader.get_env_config("discard-changes") {
            if let Some(dc) = DiscardChanges::from_str(&discard) {
                self.discard_changes = dc;
                self.sources.insert(
                    "discard-changes".to_string(),
                    ConfigSource::Environment("COMPOSER_DISCARD_CHANGES".to_string()),
                );
            }
        }

        // Cache read-only
        if let Some(readonly) = loader.get_env_bool("cache-read-only") {
            self.cache_read_only = readonly;
            self.sources.insert(
                "cache-read-only".to_string(),
                ConfigSource::Environment("COMPOSER_CACHE_READ_ONLY".to_string()),
            );
        }

        // Htaccess protect
        if let Some(htaccess) = loader.get_env_bool("htaccess-protect") {
            self.htaccess_protect = htaccess;
            self.sources.insert(
                "htaccess-protect".to_string(),
                ConfigSource::Environment("COMPOSER_HTACCESS_PROTECT".to_string()),
            );
        }
    }

    /// Resolve computed paths (e.g., {$vendor-dir}/bin)
    fn resolve_paths(&mut self, loader: &ConfigLoader) {
        if self.home_dir.is_none() {
            self.home_dir = Some(loader.get_composer_home());
        }

        if self.cache_dir.is_none() {
            self.cache_dir = Some(loader.get_cache_dir());
        }

        if self.data_dir.is_none() {
            self.data_dir = self.home_dir.clone();
        }

        if self.cache_files_dir.is_none() {
            self.cache_files_dir = Some(self.cache_dir.as_ref().unwrap().join("files"));
        }
        if self.cache_repo_dir.is_none() {
            self.cache_repo_dir = Some(self.cache_dir.as_ref().unwrap().join("repo"));
        }
        if self.cache_vcs_dir.is_none() {
            self.cache_vcs_dir = Some(self.cache_dir.as_ref().unwrap().join("vcs"));
        }

        let bin_dir_str = self.bin_dir.to_string_lossy();
        if bin_dir_str.contains("{$vendor-dir}") {
            let vendor_dir_str = self.vendor_dir.to_string_lossy();
            let resolved = bin_dir_str.replace("{$vendor-dir}", &vendor_dir_str);
            self.bin_dir = PathBuf::from(resolved);
        }
    }

    /// Get all configuration keys
    fn config_keys(&self) -> Vec<String> {
        vec![
            "vendor-dir".to_string(),
            "bin-dir".to_string(),
            "cache-dir".to_string(),
            "data-dir".to_string(),
            "process-timeout".to_string(),
            "use-include-path".to_string(),
            "preferred-install".to_string(),
            "store-auths".to_string(),
            "notify-on-install".to_string(),
            "discard-changes".to_string(),
            "optimize-autoloader".to_string(),
            "sort-packages".to_string(),
            "classmap-authoritative".to_string(),
            "apcu-autoloader".to_string(),
            "secure-http".to_string(),
            "disable-tls".to_string(),
            "lock".to_string(),
            "platform-check".to_string(),
            "github-protocols".to_string(),
            "github-domains".to_string(),
            "gitlab-domains".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.vendor_dir, PathBuf::from("vendor"));
        assert_eq!(config.bin_dir, PathBuf::from("vendor/bin"));
        assert_eq!(config.process_timeout, 300);
        assert!(config.secure_http);
        assert!(!config.disable_tls);
        assert_eq!(config.preferred_install, PreferredInstall::Dist);
        assert_eq!(config.store_auths, StoreAuths::Prompt);
    }

    #[test]
    fn test_preferred_install_from_str() {
        assert_eq!(PreferredInstall::from_str("auto"), Some(PreferredInstall::Auto));
        assert_eq!(PreferredInstall::from_str("source"), Some(PreferredInstall::Source));
        assert_eq!(PreferredInstall::from_str("dist"), Some(PreferredInstall::Dist));
        assert_eq!(PreferredInstall::from_str("invalid"), None);
    }

    #[test]
    fn test_store_auths_from_str() {
        assert_eq!(StoreAuths::from_str("true"), Some(StoreAuths::True));
        assert_eq!(StoreAuths::from_str("false"), Some(StoreAuths::False));
        assert_eq!(StoreAuths::from_str("prompt"), Some(StoreAuths::Prompt));
        assert_eq!(StoreAuths::from_str("invalid"), None);
    }

    #[test]
    fn test_discard_changes_from_str() {
        assert_eq!(DiscardChanges::from_str("true"), Some(DiscardChanges::True));
        assert_eq!(DiscardChanges::from_str("false"), Some(DiscardChanges::False));
        assert_eq!(DiscardChanges::from_str("stash"), Some(DiscardChanges::Stash));
        assert_eq!(DiscardChanges::from_str("invalid"), None);
    }

    #[test]
    fn test_platform_check_from_str() {
        assert_eq!(PlatformCheck::from_str("php-only"), Some(PlatformCheck::PhpOnly));
        assert_eq!(PlatformCheck::from_str("true"), Some(PlatformCheck::True));
        assert_eq!(PlatformCheck::from_str("false"), Some(PlatformCheck::False));
        assert_eq!(PlatformCheck::from_str("invalid"), None);
    }

    #[test]
    fn test_config_with_base_dir() {
        let config = Config::with_base_dir("/path/to/project");
        assert_eq!(config.base_dir, Some(PathBuf::from("/path/to/project")));
    }

    #[test]
    fn test_resolve_path() {
        let mut config = Config::default();
        config.base_dir = Some(PathBuf::from("/project"));

        let resolved = config.resolve_path(&PathBuf::from("vendor"));
        assert_eq!(resolved, PathBuf::from("/project/vendor"));

        let resolved = config.resolve_path(&PathBuf::from("/absolute/path"));
        assert_eq!(resolved, PathBuf::from("/absolute/path"));
    }
}
