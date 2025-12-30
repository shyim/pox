use serde::{Deserialize, Serialize};
use indexmap::IndexMap;

/// Root composer.json structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ComposerJson {
    /// Package name (vendor/name format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Package description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Package version (usually omitted, derived from VCS)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Package type: library, project, metapackage, composer-plugin
    #[serde(rename = "type", default = "default_type", skip_serializing_if = "is_default_type")]
    pub package_type: String,

    /// Keywords for search
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,

    /// Project homepage URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,

    /// Project readme file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,

    /// Release time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,

    /// License identifier(s)
    #[serde(default, skip_serializing_if = "License::is_none")]
    pub license: License,

    /// Authors
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<Author>,

    /// Support information
    #[serde(default, skip_serializing_if = "Support::is_empty")]
    pub support: Support,

    /// Funding links
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub funding: Vec<FundingLink>,

    /// Required packages
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub require: IndexMap<String, String>,

    /// Development dependencies
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub require_dev: IndexMap<String, String>,

    /// Conflicting packages
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub conflict: IndexMap<String, String>,

    /// Replaced packages
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub replace: IndexMap<String, String>,

    /// Provided packages (virtual packages)
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub provide: IndexMap<String, String>,

    /// Suggested packages
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub suggest: IndexMap<String, String>,

    /// Autoload configuration
    #[serde(default, skip_serializing_if = "Autoload::is_empty")]
    pub autoload: Autoload,

    /// Development autoload configuration
    #[serde(default, skip_serializing_if = "Autoload::is_empty")]
    pub autoload_dev: Autoload,

    /// Include paths (deprecated)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_path: Vec<String>,

    /// Target directory (deprecated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_dir: Option<String>,

    /// Minimum stability for dependencies
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_stability: Option<String>,

    /// Prefer stable versions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefer_stable: Option<bool>,

    /// Repository definitions
    #[serde(default, skip_serializing_if = "Repositories::is_none")]
    pub repositories: Repositories,

    /// Configuration options
    #[serde(default, skip_serializing_if = "ComposerConfig::is_empty")]
    pub config: ComposerConfig,

    /// Event scripts
    #[serde(default, skip_serializing_if = "Scripts::is_empty")]
    pub scripts: Scripts,

    /// Script descriptions
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub scripts_descriptions: IndexMap<String, String>,

    /// Additional data
    #[serde(default, skip_serializing_if = "is_null_value")]
    pub extra: serde_json::Value,

    /// Binaries
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bin: Vec<String>,

    /// Archive settings
    #[serde(default, skip_serializing_if = "Archive::is_empty")]
    pub archive: Archive,

    /// Abandonment notice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandoned: Option<Abandoned>,

    /// Non-feature branches regex patterns
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_feature_branches: Vec<String>,
}

impl ComposerJson {
    /// Get branch aliases from extra.branch-alias
    ///
    /// Branch aliases allow packages to map development branches to semantic versions.
    /// For example: `"dev-main": "1.0.x-dev"` makes `dev-main` appear as `1.0.x-dev`.
    ///
    /// Returns a map of source version to (alias_normalized, alias_pretty)
    pub fn get_branch_aliases(&self) -> std::collections::HashMap<String, (String, String)> {
        crate::package::parse_branch_aliases(Some(&self.extra))
    }

    /// Get inline alias from a require constraint if present
    ///
    /// Composer allows specifying aliases inline in require constraints using "as":
    /// `"vendor/package": "dev-main as 1.0.0"`
    ///
    /// Returns `Some((actual_constraint, alias_version))` if an alias is present
    pub fn get_inline_alias(constraint: &str) -> Option<(String, String)> {
        crate::package::parse_inline_alias(constraint)
    }
}

fn default_type() -> String {
    "library".to_string()
}

fn is_default_type(t: &String) -> bool {
    t == "library"
}

fn is_null_value(v: &serde_json::Value) -> bool {
    v.is_null()
}

/// License can be a single string or array of strings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(untagged)]
pub enum License {
    #[default]
    None,
    Single(String),
    Multiple(Vec<String>),
}

impl License {
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            License::None => vec![],
            License::Single(s) => vec![s.clone()],
            License::Multiple(v) => v.clone(),
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, License::None)
    }
}

/// Author information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Author {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Support information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Support {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issues: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wiki: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub irc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
}

impl Support {
    pub fn is_empty(&self) -> bool {
        self.email.is_none()
            && self.issues.is_none()
            && self.forum.is_none()
            && self.wiki.is_none()
            && self.irc.is_none()
            && self.chat.is_none()
            && self.source.is_none()
            && self.docs.is_none()
            && self.rss.is_none()
            && self.security.is_none()
    }
}

/// Funding link
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingLink {
    #[serde(rename = "type")]
    pub funding_type: String,
    pub url: String,
}

/// Autoload configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Autoload {
    /// PSR-4 autoloading
    #[serde(default, rename = "psr-4", skip_serializing_if = "IndexMap::is_empty")]
    pub psr4: IndexMap<String, AutoloadPath>,

    /// PSR-0 autoloading (deprecated)
    #[serde(default, rename = "psr-0", skip_serializing_if = "IndexMap::is_empty")]
    pub psr0: IndexMap<String, AutoloadPath>,

    /// Classmap paths
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classmap: Vec<String>,

    /// Files to always include
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,

    /// Paths to exclude from classmap
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_from_classmap: Vec<String>,
}

impl Autoload {
    pub fn is_empty(&self) -> bool {
        self.psr4.is_empty()
            && self.psr0.is_empty()
            && self.classmap.is_empty()
            && self.files.is_empty()
            && self.exclude_from_classmap.is_empty()
    }
}

/// Autoload path can be a single string or array of strings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoloadPath {
    Single(String),
    Multiple(Vec<String>),
}

impl AutoloadPath {
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            AutoloadPath::Single(s) => vec![s.clone()],
            AutoloadPath::Multiple(v) => v.clone(),
        }
    }
}

impl Default for AutoloadPath {
    fn default() -> Self {
        AutoloadPath::Multiple(vec![])
    }
}

/// Repository definitions - can be array or object
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(untagged)]
pub enum Repositories {
    #[default]
    None,
    Array(Vec<Repository>),
    Object(IndexMap<String, Repository>),
}

impl Repositories {
    pub fn as_vec(&self) -> Vec<Repository> {
        match self {
            Repositories::None => vec![],
            Repositories::Array(v) => v.clone(),
            Repositories::Object(m) => m.values().cloned().collect(),
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Repositories::None)
    }
}

/// Repository definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Repository {
    Composer {
        url: String,
        #[serde(default, skip_serializing_if = "RepositoryOptions::is_empty")]
        options: RepositoryOptions,
    },
    Vcs {
        url: String,
    },
    Git {
        url: String,
    },
    #[serde(rename = "github")]
    GitHub {
        url: String,
    },
    #[serde(rename = "gitlab")]
    GitLab {
        url: String,
    },
    #[serde(rename = "bitbucket")]
    Bitbucket {
        url: String,
    },
    Path {
        url: String,
        #[serde(default, skip_serializing_if = "PathRepositoryOptions::is_empty")]
        options: PathRepositoryOptions,
    },
    Artifact {
        url: String,
    },
    Package {
        /// Package can be a single object or an array of package objects
        package: serde_json::Value,
    },
    /// Disable a repository by name
    #[serde(untagged)]
    Disabled(bool),
}

/// Repository options
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepositoryOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl: Option<SslOptions>,
}

impl RepositoryOptions {
    pub fn is_empty(&self) -> bool {
        self.ssl.is_none()
    }
}

/// SSL options
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SslOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_peer: Option<bool>,
}

/// Path repository options
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathRepositoryOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink: Option<bool>,
}

impl PathRepositoryOptions {
    pub fn is_empty(&self) -> bool {
        self.symlink.is_none()
    }
}

/// Inline package definition for package repositories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDefinition {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist: Option<DistDefinition>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub require: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Autoload::is_empty")]
    pub autoload: Autoload,
}

/// Source definition (VCS)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDefinition {
    #[serde(rename = "type")]
    pub source_type: String,
    pub url: String,
    pub reference: String,
}

/// Dist definition (archive)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistDefinition {
    #[serde(rename = "type")]
    pub dist_type: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shasum: Option<String>,
}

/// Composer config section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ComposerConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_include_path: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_install: Option<PreferredInstall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_auths: Option<StoreAuths>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_protocols: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_oauth: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gitlab_oauth: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gitlab_token: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_basic: Option<IndexMap<String, HttpBasicAuth>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer: Option<IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<IndexMap<String, PlatformValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_files_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_repo_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_vcs_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_files_ttl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_files_maxsize: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_compat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discard_changes: Option<DiscardChanges>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoloader_suffix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimize_autoloader: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_packages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classmap_authoritative: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apcu_autoloader: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prepend_autoloader: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gitlab_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_expose_hostname: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub htaccess_protect: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_check: Option<PlatformCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secure_http: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secure_svn_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_plugins: Option<AllowPlugins>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_parent_dir: Option<bool>,
}

impl ComposerConfig {
    pub fn is_empty(&self) -> bool {
        self.process_timeout.is_none()
            && self.use_include_path.is_none()
            && self.preferred_install.is_none()
            && self.store_auths.is_none()
            && self.github_protocols.is_none()
            && self.github_oauth.is_none()
            && self.gitlab_oauth.is_none()
            && self.gitlab_token.is_none()
            && self.http_basic.is_none()
            && self.bearer.is_none()
            && self.platform.is_none()
            && self.vendor_dir.is_none()
            && self.bin_dir.is_none()
            && self.data_dir.is_none()
            && self.cache_dir.is_none()
            && self.cache_files_dir.is_none()
            && self.cache_repo_dir.is_none()
            && self.cache_vcs_dir.is_none()
            && self.cache_files_ttl.is_none()
            && self.cache_files_maxsize.is_none()
            && self.cache_read_only.is_none()
            && self.bin_compat.is_none()
            && self.discard_changes.is_none()
            && self.autoloader_suffix.is_none()
            && self.optimize_autoloader.is_none()
            && self.sort_packages.is_none()
            && self.classmap_authoritative.is_none()
            && self.apcu_autoloader.is_none()
            && self.prepend_autoloader.is_none()
            && self.github_domains.is_none()
            && self.gitlab_domains.is_none()
            && self.github_expose_hostname.is_none()
            && self.htaccess_protect.is_none()
            && self.lock.is_none()
            && self.platform_check.is_none()
            && self.archive_format.is_none()
            && self.archive_dir.is_none()
            && self.secure_http.is_none()
            && self.secure_svn_domains.is_none()
            && self.allow_plugins.is_none()
            && self.use_parent_dir.is_none()
    }
}

/// Preferred install method
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PreferredInstall {
    Global(String),
    PerPackage(IndexMap<String, String>),
}

impl Default for PreferredInstall {
    fn default() -> Self {
        PreferredInstall::Global("dist".to_string())
    }
}

/// Store authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StoreAuths {
    Bool(bool),
    Prompt(String),
}

impl Default for StoreAuths {
    fn default() -> Self {
        StoreAuths::Prompt("prompt".to_string())
    }
}

/// HTTP basic auth credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpBasicAuth {
    pub username: String,
    pub password: String,
}

/// Platform override value - can be false to disable
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformValue {
    Version(String),
    Disabled(bool),
}

/// Discard changes mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiscardChanges {
    Bool(bool),
    Mode(String), // "stash" or "discard"
}

impl Default for DiscardChanges {
    fn default() -> Self {
        DiscardChanges::Bool(false)
    }
}

/// Platform check mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformCheck {
    Bool(bool),
    Mode(String), // "php-only"
}

impl Default for PlatformCheck {
    fn default() -> Self {
        PlatformCheck::Bool(true)
    }
}

/// Allow plugins setting
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AllowPlugins {
    Bool(bool),
    List(IndexMap<String, bool>),
}

impl Default for AllowPlugins {
    fn default() -> Self {
        AllowPlugins::Bool(true)
    }
}

/// Scripts configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Scripts {
    // Composer events
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub pre_install_cmd: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub post_install_cmd: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub pre_update_cmd: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub post_update_cmd: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub post_status_cmd: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub pre_archive_cmd: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub post_archive_cmd: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub pre_autoload_dump: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub post_autoload_dump: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub post_root_package_install: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub post_create_project_cmd: ScriptValue,
    #[serde(default, skip_serializing_if = "ScriptValue::is_empty")]
    pub pre_operations_exec: ScriptValue,

    // Custom scripts
    #[serde(flatten)]
    pub custom: IndexMap<String, ScriptValue>,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self.pre_install_cmd.is_empty()
            && self.post_install_cmd.is_empty()
            && self.pre_update_cmd.is_empty()
            && self.post_update_cmd.is_empty()
            && self.post_status_cmd.is_empty()
            && self.pre_archive_cmd.is_empty()
            && self.post_archive_cmd.is_empty()
            && self.pre_autoload_dump.is_empty()
            && self.post_autoload_dump.is_empty()
            && self.post_root_package_install.is_empty()
            && self.post_create_project_cmd.is_empty()
            && self.pre_operations_exec.is_empty()
            && self.custom.is_empty()
    }
}

/// Script value - can be string or array
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(untagged)]
pub enum ScriptValue {
    #[default]
    None,
    Single(String),
    Multiple(Vec<String>),
}

impl ScriptValue {
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            ScriptValue::None => vec![],
            ScriptValue::Single(s) => vec![s.clone()],
            ScriptValue::Multiple(v) => v.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            ScriptValue::None => true,
            ScriptValue::Single(_) => false,
            ScriptValue::Multiple(v) => v.is_empty(),
        }
    }
}

/// Archive configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Archive {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

impl Archive {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.exclude.is_empty()
    }
}

/// Abandonment notice
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Abandoned {
    Bool(bool),
    Replacement(String),
}

impl ComposerJson {
    /// Get the full package name (vendor/package)
    pub fn package_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Check if this is a root/project package
    pub fn is_root(&self) -> bool {
        self.package_type == "project"
    }

    /// Get all dependencies (require + require-dev)
    pub fn all_dependencies(&self) -> IndexMap<String, String> {
        let mut deps = self.require.clone();
        deps.extend(self.require_dev.clone());
        deps
    }

    /// Get license as vector
    pub fn licenses(&self) -> Vec<String> {
        self.license.as_vec()
    }
}
