use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// The main phpx configuration file structure (phpx.toml)
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PhpxConfig {
    /// PHP runtime configuration
    pub php: PhpConfig,

    /// Server configuration
    pub server: ServerConfig,
}

/// PHP-specific configuration
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PhpConfig {
    /// PHP version requirement (e.g., "8.3", "^8.2", "8.3.15")
    /// If not specified, uses the embedded/default version
    pub version: Option<String>,

    /// PHP INI settings (e.g., memory_limit = "256M")
    #[serde(default)]
    pub ini: HashMap<String, String>,
}

/// Development server configuration
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Host to bind to
    pub host: Option<String>,

    /// Port to listen on
    pub port: Option<u16>,

    /// Document root directory
    pub document_root: Option<String>,

    /// Router script path
    pub router: Option<String>,

    /// Worker script path
    pub worker: Option<String>,

    /// Number of workers
    pub workers: Option<usize>,

    /// Watch patterns for file changes
    #[serde(default)]
    pub watch: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: None,
            port: None,
            document_root: None,
            router: None,
            worker: None,
            workers: None,
            watch: Vec::new(),
        }
    }
}

impl PhpxConfig {
    /// Load configuration from phpx.toml, searching upward from the given directory
    pub fn load(start_dir: &Path) -> Result<Option<Self>> {
        let mut current = start_dir.to_path_buf();

        loop {
            let config_path = current.join("phpx.toml");

            if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                let config: PhpxConfig = toml::from_str(&content)?;
                return Ok(Some(config));
            }

            // Move to parent directory
            if !current.pop() {
                // Reached filesystem root, no config found
                return Ok(None);
            }
        }
    }

    /// Load configuration by searching upward from the current working directory
    pub fn load_from_cwd() -> Result<Option<Self>> {
        let cwd = std::env::current_dir()?;
        Self::load(&cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_config() {
        let config: PhpxConfig = toml::from_str("").unwrap();
        assert!(config.php.ini.is_empty());
    }

    #[test]
    fn test_parse_php_ini() {
        let toml = r#"
[php.ini]
memory_limit = "256M"
max_execution_time = "30"
display_errors = "On"
"#;
        let config: PhpxConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.php.ini.get("memory_limit"), Some(&"256M".to_string()));
        assert_eq!(config.php.ini.get("max_execution_time"), Some(&"30".to_string()));
        assert_eq!(config.php.ini.get("display_errors"), Some(&"On".to_string()));
    }

    #[test]
    fn test_parse_php_version() {
        let toml = r#"
[php]
version = "8.3"

[php.ini]
memory_limit = "256M"
"#;
        let config: PhpxConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.php.version, Some("8.3".to_string()));
        assert_eq!(config.php.ini.get("memory_limit"), Some(&"256M".to_string()));
    }

    #[test]
    fn test_parse_php_version_constraint() {
        let toml = r#"
[php]
version = "^8.2"
"#;
        let config: PhpxConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.php.version, Some("^8.2".to_string()));
    }

    #[test]
    fn test_parse_server_config() {
        let toml = r#"
[server]
host = "0.0.0.0"
port = 9000
document_root = "public"
router = "index.php"
worker = "worker.php"
workers = 4
watch = ["**/*.php", "config/**/*"]
"#;
        let config: PhpxConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.server.host, Some("0.0.0.0".to_string()));
        assert_eq!(config.server.port, Some(9000));
        assert_eq!(config.server.document_root, Some("public".to_string()));
        assert_eq!(config.server.router, Some("index.php".to_string()));
        assert_eq!(config.server.worker, Some("worker.php".to_string()));
        assert_eq!(config.server.workers, Some(4));
        assert_eq!(config.server.watch, vec!["**/*.php", "config/**/*"]);
    }

}
