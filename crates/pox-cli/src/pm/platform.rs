//! Platform detection for PHP version and extensions.
//!
//! This module detects the installed PHP version and extensions
//! and creates virtual packages that can be used by the dependency solver.

use pox_pm::Package;

/// Information about the PHP platform
#[derive(Debug, Clone)]
pub struct PlatformInfo {
    /// PHP version string (e.g., "8.4.0")
    pub php_version: String,
    /// PHP version ID (e.g., 80400)
    #[allow(dead_code)]
    pub php_version_id: i32,
    /// List of loaded extensions (lowercase)
    pub extensions: Vec<String>,
}

impl PlatformInfo {
    /// Detect the current PHP platform using the embedded PHP runtime
    pub fn detect() -> Self {
        let version = pox_embed::Php::version();

        // Get loaded extensions
        let extensions = pox_embed::Php::get_loaded_extensions()
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect();

        Self {
            php_version: version.version.to_string(),
            php_version_id: version.version_id,
            extensions,
        }
    }

    /// Check if an extension is available
    #[allow(dead_code)]
    pub fn has_extension(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();
        self.extensions.iter().any(|e| e == &name_lower)
    }

    /// Create virtual packages representing the platform
    ///
    /// Returns packages for:
    /// - `php` with the current PHP version
    /// - `php-64bit`, `php-ipv6`, `php-zts`, `php-debug` variants
    /// - `ext-*` for each loaded extension
    /// - `composer`, `composer-runtime-api`, `composer-plugin-api`
    /// - `lib-*` for common libraries
    pub fn to_packages(&self) -> Vec<Package> {
        let mut packages = Vec::new();

        // PHP version package
        let php_pkg = Package::new("php", &self.php_version);
        packages.push(php_pkg);

        // PHP 64-bit package (always on 64-bit platforms)
        #[cfg(target_pointer_width = "64")]
        {
            let php64_pkg = Package::new("php-64bit", &self.php_version);
            packages.push(php64_pkg);
        }

        // PHP IPv6 support - assume available on modern systems
        let php_ipv6_pkg = Package::new("php-ipv6", &self.php_version);
        packages.push(php_ipv6_pkg);

        // PHP ZTS (Thread Safety)
        if pox_embed::Php::is_zts() {
            let php_zts_pkg = Package::new("php-zts", &self.php_version);
            packages.push(php_zts_pkg);
        }

        // PHP Debug
        if pox_embed::Php::is_debug() {
            let php_debug_pkg = Package::new("php-debug", &self.php_version);
            packages.push(php_debug_pkg);
        }

        // Extension packages
        for ext in &self.extensions {
            // Skip standard and Core as they're built-in
            if ext == "standard" || ext == "core" {
                continue;
            }

            let ext_name = format!("ext-{}", ext);
            // Extensions use the PHP version as their version
            let ext_pkg = Package::new(&ext_name, &self.php_version);
            packages.push(ext_pkg);
        }

        // Composer packages - these are virtual packages that indicate Composer runtime compatibility
        // Using version 2.99.99 to indicate full Composer 2.x compatibility
        let composer_pkg = Package::new("composer", "2.99.99");
        packages.push(composer_pkg);

        // Composer Runtime API - version 2.2.2 is current stable
        let runtime_api_pkg = Package::new("composer-runtime-api", "2.2.2");
        packages.push(runtime_api_pkg);

        // Composer Plugin API - version 2.6.0 is current stable
        let plugin_api_pkg = Package::new("composer-plugin-api", "2.6.0");
        packages.push(plugin_api_pkg);

        // Add common lib-* packages based on loaded extensions
        self.add_library_packages(&mut packages);

        packages
    }

    /// Add lib-* packages based on loaded extensions
    fn add_library_packages(&self, packages: &mut Vec<Package>) {
        // ICU library (from intl extension)
        if let Some(version) = pox_embed::Php::icu_version() {
            if !version.is_empty() {
                packages.push(Package::new("lib-icu", version));
            }
        }

        // libxml
        if let Some(version) = pox_embed::Php::libxml_version() {
            if !version.is_empty() {
                packages.push(Package::new("lib-libxml", version));
            }
        }

        // OpenSSL - parse version from text like "OpenSSL 3.0.2 15 Mar 2022"
        if let Some(version_text) = pox_embed::Php::openssl_version() {
            if let Some(version) = parse_openssl_version(version_text) {
                packages.push(Package::new("lib-openssl", &version));
            }
        }

        // PCRE
        if let Some(version) = pox_embed::Php::pcre_version() {
            if !version.is_empty() {
                packages.push(Package::new("lib-pcre", version));
            }
        }

        // zlib
        if let Some(version) = pox_embed::Php::zlib_version() {
            if !version.is_empty() {
                packages.push(Package::new("lib-zlib", version));
            }
        }

        // curl
        if let Some(version) = pox_embed::Php::curl_version() {
            if !version.is_empty() {
                packages.push(Package::new("lib-curl", version));
            }
        }
    }
}

/// Parse OpenSSL version from version text
fn parse_openssl_version(version_text: &str) -> Option<String> {
    // Format: "OpenSSL 3.0.2 15 Mar 2022" or "LibreSSL 3.3.6"
    let parts: Vec<&str> = version_text.split_whitespace().collect();
    if parts.len() >= 2 {
        Some(parts[1].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detect() {
        let platform = PlatformInfo::detect();
        assert!(!platform.php_version.is_empty());
        assert!(platform.php_version_id > 0);
        // Core is always loaded
        assert!(platform.has_extension("core"));
    }

    #[test]
    fn test_to_packages() {
        let platform = PlatformInfo::detect();
        let packages = platform.to_packages();

        // Should have at least php package
        assert!(!packages.is_empty());
        assert!(packages.iter().any(|p| p.name == "php"));
    }
}
