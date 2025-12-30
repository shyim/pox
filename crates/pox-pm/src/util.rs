//! Utility functions for the package manager.

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
