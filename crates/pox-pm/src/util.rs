//! Utility functions for the package manager.

use md5::{Md5, Digest};
use serde_json::Value;

/// Compute the content hash for a composer.json file.
/// This matches Composer's algorithm:
/// 1. Parse the JSON
/// 2. Extract relevant keys (name, version, require, etc.)
/// 3. Sort keys alphabetically
/// 4. JSON encode (compact, no pretty print)
/// 5. MD5 hash
pub fn compute_content_hash(json_content: &str) -> String {
    let json_value: Value = match serde_json::from_str(json_content) {
        Ok(v) => v,
        Err(_) => return "0".repeat(32),
    };

    let mut relevant_keys = vec![
        "name", "version", "require", "require-dev", "conflict",
        "replace", "provide", "minimum-stability", "prefer-stable",
        "repositories", "extra"
    ];
    relevant_keys.sort();

    let mut relevant_map = serde_json::Map::new();

    if let Some(obj) = json_value.as_object() {
        for key in &relevant_keys {
            if let Some(value) = obj.get(*key) {
                relevant_map.insert(key.to_string(), value.clone());
            }
        }
    }

    // Include config.platform if present
    if let Some(config) = json_value.get("config") {
        if let Some(platform) = config.get("platform") {
            if let Some(obj) = platform.as_object() {
                if !obj.is_empty() {
                    let mut config_obj = serde_json::Map::new();
                    config_obj.insert("platform".to_string(), platform.clone());
                    relevant_map.insert("config".to_string(), Value::Object(config_obj));
                }
            }
        }
    }

    let json_output = match serde_json::to_string(&Value::Object(relevant_map)) {
        Ok(s) => s,
        Err(_) => return "0".repeat(32),
    };

    // Composer escapes forward slashes in JSON
    let json_with_escaped_slashes = json_output.replace('/', "\\/");

    let mut hasher = Md5::new();
    hasher.update(json_with_escaped_slashes.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

/// Check if a package name represents a platform package.
///
/// Platform packages are virtual packages that represent the PHP runtime
/// and its extensions. They include:
/// - `php` - The PHP interpreter itself
/// - `php-64bit`, `php-ipv6`, `php-zts`, `php-debug` - PHP capability packages
/// - `ext-*` - PHP extensions (e.g., `ext-json`, `ext-mbstring`)
/// - `lib-*` - System libraries (e.g., `lib-libxml`)
/// - `composer`, `composer-runtime-api`, `composer-plugin-api` - Composer packages
///
/// # Examples
///
/// ```
/// use pox_pm::util::is_platform_package;
///
/// assert!(is_platform_package("php"));
/// assert!(is_platform_package("php-64bit"));
/// assert!(is_platform_package("ext-json"));
/// assert!(is_platform_package("lib-libxml"));
/// assert!(is_platform_package("composer"));
/// assert!(is_platform_package("composer-runtime-api"));
///
/// // These are NOT platform packages
/// assert!(!is_platform_package("phpstan/phpstan"));
/// assert!(!is_platform_package("phpunit/phpunit"));
/// assert!(!is_platform_package("symfony/console"));
/// ```
pub fn is_platform_package(name: &str) -> bool {
    // Exact matches for php and its variants
    name == "php"
        || name == "php-64bit"
        || name == "php-ipv6"
        || name == "php-zts"
        || name == "php-debug"
        // Extension and library prefixes
        || name.starts_with("ext-")
        || name.starts_with("lib-")
        // Composer virtual packages
        || name == "composer"
        || name == "composer-runtime-api"
        || name == "composer-plugin-api"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_platform_package_php() {
        assert!(is_platform_package("php"));
    }

    #[test]
    fn test_is_platform_package_php_variants() {
        assert!(is_platform_package("php-64bit"));
        assert!(is_platform_package("php-ipv6"));
        assert!(is_platform_package("php-zts"));
        assert!(is_platform_package("php-debug"));
    }

    #[test]
    fn test_is_platform_package_extensions() {
        assert!(is_platform_package("ext-json"));
        assert!(is_platform_package("ext-mbstring"));
        assert!(is_platform_package("ext-pdo"));
        assert!(is_platform_package("ext-curl"));
    }

    #[test]
    fn test_is_platform_package_libraries() {
        assert!(is_platform_package("lib-libxml"));
        assert!(is_platform_package("lib-openssl"));
        assert!(is_platform_package("lib-pcre"));
    }

    #[test]
    fn test_is_platform_package_composer() {
        assert!(is_platform_package("composer"));
        assert!(is_platform_package("composer-runtime-api"));
        assert!(is_platform_package("composer-plugin-api"));
    }

    #[test]
    fn test_is_platform_package_not_platform() {
        // Packages that start with "php" but are NOT platform packages
        assert!(!is_platform_package("phpstan/phpstan"));
        assert!(!is_platform_package("phpunit/phpunit"));
        assert!(!is_platform_package("phpdocumentor/phpdocumentor"));
        assert!(!is_platform_package("php-cs-fixer/shim"));

        // Regular packages
        assert!(!is_platform_package("symfony/console"));
        assert!(!is_platform_package("laravel/framework"));
        assert!(!is_platform_package("doctrine/orm"));

        // Packages that might be confused with platform packages
        assert!(!is_platform_package("ext")); // Not ext-*
        assert!(!is_platform_package("lib")); // Not lib-*
        assert!(!is_platform_package("extension-helper"));
        assert!(!is_platform_package("library-package"));
    }

    #[test]
    fn test_is_platform_package_phpunit_packages() {
        assert!(!is_platform_package("phpunit/phpunit"));
        assert!(!is_platform_package("phpunit/php-code-coverage"));
        assert!(!is_platform_package("phpunit/php-file-iterator"));
        assert!(!is_platform_package("phpunit/php-text-template"));
        assert!(!is_platform_package("phpunit/php-timer"));
        assert!(!is_platform_package("phpunit/php-invoker"));
    }

    #[test]
    fn test_is_platform_package_case_sensitivity() {
        // Platform package names are case-sensitive (lowercase)
        // These should NOT match as platform packages
        assert!(!is_platform_package("PHP"));
        assert!(!is_platform_package("Ext-json"));
        assert!(!is_platform_package("COMPOSER"));
    }
}
