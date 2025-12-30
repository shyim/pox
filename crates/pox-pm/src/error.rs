use thiserror::Error;

#[derive(Error, Debug)]
pub enum ComposerError {
    // JSON/parsing errors
    #[error("Failed to parse composer.json: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Invalid composer.json: {message}")]
    InvalidManifest { message: String },

    // Package errors
    #[error("Package not found: {name}")]
    PackageNotFound { name: String },

    #[error("Version not found: {name}@{version}")]
    VersionNotFound { name: String, version: String },

    // Repository errors
    #[error("Repository error: {0}")]
    Repository(String),

    // Network errors
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    // IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    // Solver errors
    #[error("Could not resolve dependencies: {0}")]
    DependencyResolution(String),

    // Download errors
    #[error("Download failed for {package}: {reason}")]
    DownloadFailed { package: String, reason: String },

    #[error("Checksum mismatch for {package}")]
    ChecksumMismatch { package: String },

    // Installation errors
    #[error("Installation failed: {0}")]
    InstallationFailed(String),

    // Config errors
    #[error("Configuration error: {0}")]
    Config(String),

    // Version constraint errors
    #[error("Invalid version constraint: {0}")]
    InvalidConstraint(String),

    // Lock file errors
    #[error("Lock file is out of sync with composer.json")]
    LockFileOutOfSync,

    // Git errors
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),
}

pub type Result<T> = std::result::Result<T, ComposerError>;
