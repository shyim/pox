/// Integration tests for the configuration system
///
/// These tests verify that the configuration system works correctly
/// when loading from files and environment variables.

use pox_pm::config::{Config, ConfigLoader, PreferredInstall, StoreAuths};
use std::env;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_config_defaults() {
    let config = Config::default();

    assert_eq!(config.vendor_dir, PathBuf::from("vendor"));
    assert_eq!(config.bin_dir, PathBuf::from("vendor/bin"));
    assert_eq!(config.process_timeout, 300);
    assert_eq!(config.cache_ttl, 15552000);
    assert_eq!(config.cache_files_maxsize, 300 * 1024 * 1024);
    assert!(config.secure_http);
    assert!(!config.disable_tls);
    assert!(config.lock);
    assert_eq!(config.preferred_install, PreferredInstall::Dist);
    assert_eq!(config.store_auths, StoreAuths::Prompt);
    assert_eq!(config.github_protocols, vec!["https", "ssh", "git"]);
    assert_eq!(config.github_domains, vec!["github.com"]);
}

#[test]
fn test_config_with_base_dir() {
    let config = Config::with_base_dir("/project/path");

    assert_eq!(config.base_dir(), Some(PathBuf::from("/project/path").as_path()));
    assert_eq!(config.get_vendor_dir(), PathBuf::from("/project/path/vendor"));
    assert_eq!(config.get_bin_dir(), PathBuf::from("/project/path/vendor/bin"));
}

#[test]
fn test_config_loader_directories() {
    let loader = ConfigLoader::new(false);

    let home = loader.get_composer_home();
    assert!(home.is_absolute() || home.starts_with(".composer"));

    let cache = loader.get_cache_dir();
    assert!(cache.is_absolute() || cache.ends_with("cache"));
}

#[test]
fn test_config_loader_env_disabled() {
    let loader = ConfigLoader::new(false);

    // Should return None when environment is disabled
    assert_eq!(loader.get_composer_env("COMPOSER_HOME"), None);
    assert_eq!(loader.get_env_config("vendor-dir"), None);
}

#[test]
fn test_config_loader_env_enabled() {
    // Set test environment variables
    env::set_var("COMPOSER_TEST_VAR", "test_value");
    env::set_var("COMPOSER_VENDOR_DIR", "/custom/vendor");

    let loader = ConfigLoader::new(true);

    assert_eq!(loader.get_composer_env("COMPOSER_TEST_VAR"), Some("test_value".to_string()));
    assert_eq!(loader.get_env_config("vendor-dir"), Some("/custom/vendor".to_string()));
    assert_eq!(loader.get_env_path("vendor-dir"), Some(PathBuf::from("/custom/vendor")));

    // Clean up
    env::remove_var("COMPOSER_TEST_VAR");
    env::remove_var("COMPOSER_VENDOR_DIR");
}

#[test]
fn test_load_empty_config_file() {
    let temp_dir = TempDir::new().unwrap();
    let config_file = temp_dir.path().join("config.json");

    fs::write(&config_file, "{}").unwrap();

    let loader = ConfigLoader::new(false);
    let config = loader.load_config_file(&config_file).unwrap();

    assert!(config.config.is_none());
    assert!(config.repositories.is_none());
}

#[test]
fn test_load_config_with_values() {
    let temp_dir = TempDir::new().unwrap();
    let config_file = temp_dir.path().join("config.json");

    let config_json = r#"{
        "config": {
            "vendor-dir": "lib/vendor",
            "process-timeout": 600,
            "optimize-autoloader": true,
            "platform": {
                "php": "8.2.0"
            }
        }
    }"#;

    fs::write(&config_file, config_json).unwrap();

    let loader = ConfigLoader::new(false);
    let raw_config = loader.load_config_file(&config_file).unwrap();

    assert!(raw_config.config.is_some());
    let config_map = raw_config.config.unwrap();
    assert!(config_map.contains_key("vendor-dir"));
    assert!(config_map.contains_key("process-timeout"));
    assert!(config_map.contains_key("optimize-autoloader"));
}

#[test]
fn test_build_config_no_files() {
    // Build config without any project directory (uses only defaults and global config if exists)
    let config = Config::build(None::<&str>, false).unwrap();

    // Should have defaults
    assert_eq!(config.vendor_dir, PathBuf::from("vendor"));
    assert_eq!(config.process_timeout, 300);
    assert!(config.secure_http);
}

#[test]
fn test_build_config_with_project_dir() {
    let temp_dir = TempDir::new().unwrap();
    let composer_json = temp_dir.path().join("composer.json");

    let json_content = r#"{
        "name": "test/project",
        "config": {
            "vendor-dir": "deps",
            "optimize-autoloader": true,
            "sort-packages": true
        }
    }"#;

    fs::write(&composer_json, json_content).unwrap();

    let config = Config::build(Some(temp_dir.path()), false).unwrap();

    // Should have project config values
    assert_eq!(config.vendor_dir, PathBuf::from("deps"));
    assert!(config.optimize_autoloader);
    assert!(config.sort_packages);

    // Check sources
    assert!(config.get_source("vendor-dir").is_some());
    assert!(config.get_source("optimize-autoloader").is_some());
}

#[test]
fn test_config_env_overrides() {
    env::set_var("COMPOSER_PROCESS_TIMEOUT", "900");
    env::set_var("COMPOSER_VENDOR_DIR", "/override/vendor");

    let config = Config::build(None::<&str>, true).unwrap();

    assert_eq!(config.process_timeout, 900);
    assert_eq!(config.vendor_dir, PathBuf::from("/override/vendor"));

    // Clean up
    env::remove_var("COMPOSER_PROCESS_TIMEOUT");
    env::remove_var("COMPOSER_VENDOR_DIR");
}

#[test]
fn test_config_path_resolution() {
    let temp_dir = TempDir::new().unwrap();
    let project_path = temp_dir.path();

    let mut config = Config::with_base_dir(project_path);
    config.vendor_dir = PathBuf::from("custom/vendor");

    let resolved = config.get_vendor_dir();
    assert_eq!(resolved, project_path.join("custom/vendor"));
}

#[test]
fn test_config_serialization() {
    let config = Config::default();

    // Should be able to serialize to JSON
    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("vendor-dir"));

    // Should be able to deserialize
    let deserialized: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.vendor_dir, config.vendor_dir);
    assert_eq!(deserialized.process_timeout, config.process_timeout);
}

#[test]
fn test_preferred_install_variants() {
    let mut config = Config::default();

    config.preferred_install = PreferredInstall::Auto;
    assert_eq!(config.preferred_install, PreferredInstall::Auto);

    config.preferred_install = PreferredInstall::Source;
    assert_eq!(config.preferred_install, PreferredInstall::Source);

    config.preferred_install = PreferredInstall::Dist;
    assert_eq!(config.preferred_install, PreferredInstall::Dist);
}

#[test]
fn test_platform_overrides() {
    let mut config = Config::default();

    config.platform.insert("php".to_string(), "8.2.0".to_string());
    config.platform.insert("ext-mbstring".to_string(), "*".to_string());

    assert_eq!(config.platform.get("php"), Some(&"8.2.0".to_string()));
    assert_eq!(config.platform.get("ext-mbstring"), Some(&"*".to_string()));
}

#[test]
fn test_authentication_config() {
    use pox_pm::config::HttpBasicAuth;

    let mut config = Config::default();

    config.http_basic.insert(
        "example.com".to_string(),
        HttpBasicAuth {
            username: "user".to_string(),
            password: "pass".to_string(),
        },
    );

    config.github_oauth.insert("github.com".to_string(), "token123".to_string());
    config.bearer.insert("api.example.com".to_string(), "bearer_token".to_string());

    assert!(config.http_basic.contains_key("example.com"));
    assert!(config.github_oauth.contains_key("github.com"));
    assert!(config.bearer.contains_key("api.example.com"));
}

#[test]
fn test_cache_configuration() {
    let config = Config::default();

    assert_eq!(config.cache_ttl, 15552000); // 6 months
    assert_eq!(config.cache_files_maxsize, 300 * 1024 * 1024); // 300 MB
    assert!(!config.cache_read_only);

    let config = Config::build(None::<&str>, false).unwrap();

    // Cache directories should be set
    assert!(config.cache_dir.is_some());
    assert!(config.cache_files_dir.is_some());
    assert!(config.cache_repo_dir.is_some());
    assert!(config.cache_vcs_dir.is_some());
}
