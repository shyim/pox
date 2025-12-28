//! Autoload generator - creates PHP autoloader files.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use indexmap::IndexMap;

use md5::{Md5, Digest};
use regex::Regex;

use crate::package::Autoload;
use crate::Result;

use super::classmap::ClassMapGenerator;

/// Sort packages by dependency weight (topological sort).
/// Packages that are dependencies come first, alphabetical by name as tie-breaker.
fn sort_packages_by_dependency(packages: &[PackageAutoload]) -> Vec<PackageAutoload> {
    if packages.is_empty() {
        return Vec::new();
    }

    // Build a map of package names for quick lookup
    let package_names: HashSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();

    // Calculate weight for each package (number of packages that depend on it)
    let mut weights: HashMap<&str, usize> = HashMap::new();
    for pkg in packages {
        weights.entry(&pkg.name).or_insert(0);
    }

    // For each package, increase weight of its dependencies
    for pkg in packages {
        for dep in &pkg.requires {
            // Only count dependencies that are in our package list
            if package_names.contains(dep.as_str()) {
                *weights.entry(dep.as_str()).or_insert(0) += 1;
            }
        }
    }

    // Sort by weight (descending - most depended-on first), then by name (ascending)
    let mut sorted: Vec<_> = packages.to_vec();
    sorted.sort_by(|a, b| {
        let weight_a = weights.get(a.name.as_str()).unwrap_or(&0);
        let weight_b = weights.get(b.name.as_str()).unwrap_or(&0);

        // Higher weight comes first
        match weight_b.cmp(weight_a) {
            std::cmp::Ordering::Equal => a.name.cmp(&b.name), // Alphabetical tie-breaker
            other => other,
        }
    });

    sorted
}

/// Configuration for autoload generation
#[derive(Debug, Clone)]
pub struct AutoloadConfig {
    /// Vendor directory
    pub vendor_dir: PathBuf,
    /// Base directory (project root)
    pub base_dir: PathBuf,
    /// Whether to optimize autoloader (authoritative classmap)
    pub optimize: bool,
    /// Whether to use APCu for caching
    pub apcu: bool,
    /// Whether to generate authoritative classmap
    pub authoritative: bool,
    /// Suffix for class names (content-hash from lock file)
    pub suffix: Option<String>,
}

impl Default for AutoloadConfig {
    fn default() -> Self {
        Self {
            vendor_dir: PathBuf::from("vendor"),
            base_dir: PathBuf::from("."),
            optimize: false,
            apcu: false,
            authoritative: false,
            suffix: None,
        }
    }
}

/// Package with autoload information for generation
#[derive(Debug, Clone)]
pub struct PackageAutoload {
    /// Package name
    pub name: String,
    /// Autoload configuration
    pub autoload: Autoload,
    /// Install path relative to vendor dir
    pub install_path: String,
    /// Package dependencies (required packages) - used for sorting
    pub requires: Vec<String>,
    /// Pretty version string (e.g., "1.2.3", "dev-main")
    pub pretty_version: Option<String>,
    /// Normalized version string (e.g., "1.2.3.0")
    pub version: Option<String>,
    /// VCS reference (commit hash, tag)
    pub reference: Option<String>,
    /// Package type (library, project, etc.)
    pub package_type: String,
    /// Whether this is a dev requirement
    pub dev_requirement: bool,
    /// Version aliases
    pub aliases: Vec<String>,
    /// Packages that this package replaces (name -> version constraint)
    pub replaces: IndexMap<String, String>,
    /// Packages that this package provides (name -> version constraint)
    pub provides: IndexMap<String, String>,
}

impl PackageAutoload {
    /// Returns true if this is a metapackage (no files, only dependencies)
    pub fn is_metapackage(&self) -> bool {
        self.package_type == crate::package::package_type::METAPACKAGE
    }
}

impl Default for PackageAutoload {
    fn default() -> Self {
        Self {
            name: String::new(),
            autoload: Autoload::default(),
            install_path: String::new(),
            requires: Vec::new(),
            pretty_version: None,
            version: None,
            reference: None,
            package_type: "library".to_string(),
            dev_requirement: false,
            aliases: Vec::new(),
            replaces: IndexMap::new(),
            provides: IndexMap::new(),
        }
    }
}

/// Root package information for installed.php
#[derive(Debug, Clone, Default)]
pub struct RootPackageInfo {
    /// Package name (vendor/package format)
    pub name: String,
    /// Pretty version string
    pub pretty_version: String,
    /// Normalized version string
    pub version: String,
    /// VCS reference
    pub reference: Option<String>,
    /// Package type
    pub package_type: String,
    /// Version aliases
    pub aliases: Vec<String>,
    /// Whether dev dependencies are installed
    pub dev_mode: bool,
}

/// Autoload generator
pub struct AutoloadGenerator {
    config: AutoloadConfig,
    classmap_generator: ClassMapGenerator,
}

impl AutoloadGenerator {
    /// Create a new autoload generator
    pub fn new(config: AutoloadConfig) -> Self {
        Self {
            config,
            classmap_generator: ClassMapGenerator::new(),
        }
    }

    /// Get the suffix for class names
    fn get_suffix(&self) -> String {
        self.config.suffix.clone().unwrap_or_else(|| {
            // Generate a random suffix if none provided
            let mut hasher = Md5::new();
            hasher.update(format!("{:?}", std::time::SystemTime::now()).as_bytes());
            format!("{:x}", hasher.finalize())[..16].to_string()
        })
    }

    /// Collect and compile exclude-from-classmap patterns from all packages
    fn collect_exclude_patterns(&self, packages: &[PackageAutoload], root_autoload: Option<&Autoload>) -> Vec<Regex> {
        let mut patterns = Vec::new();

        // Collect patterns from packages
        for pkg in packages {
            for pattern in &pkg.autoload.exclude_from_classmap {
                if let Some(regex) = self.compile_exclude_pattern(pattern, &pkg.install_path, false) {
                    patterns.push(regex);
                }
            }
        }

        // Collect patterns from root autoload
        if let Some(autoload) = root_autoload {
            for pattern in &autoload.exclude_from_classmap {
                if let Some(regex) = self.compile_exclude_pattern(pattern, "", true) {
                    patterns.push(regex);
                }
            }
        }

        patterns
    }

    /// Compile an exclude-from-classmap pattern to a regex
    /// Handles wildcards (* and **) similar to Composer
    fn compile_exclude_pattern(&self, pattern: &str, install_path: &str, is_root: bool) -> Option<Regex> {
        // Normalize path separators
        let pattern = pattern.replace('\\', "/").trim_matches('/').to_string();

        // Build the full path pattern
        let full_pattern = if is_root {
            // For root package, pattern is relative to base_dir
            let base = self.config.base_dir.to_string_lossy().replace('\\', "/");
            format!("{}/{}", base.trim_end_matches('/'), pattern)
        } else {
            // For packages, pattern is relative to the package install path
            let vendor = self.config.vendor_dir.to_string_lossy().replace('\\', "/");
            format!("{}/{}/{}", vendor.trim_end_matches('/'), install_path, pattern)
        };

        // Escape regex special characters, but preserve * and **
        let escaped = regex::escape(&full_pattern);

        // Convert wildcards:
        // ** matches any characters including /
        // * matches any characters except /
        let regex_pattern = escaped
            .replace(r"\*\*", ".*")  // ** -> match anything
            .replace(r"\*", "[^/]*"); // * -> match anything except /

        // Compile the regex
        Regex::new(&regex_pattern).ok()
    }

    /// Generate autoloader for installed packages
    pub fn generate(&self, packages: &[PackageAutoload], root_autoload: Option<&Autoload>, root_package: Option<&RootPackageInfo>) -> Result<()> {
        let composer_dir = self.config.vendor_dir.join("composer");
        std::fs::create_dir_all(&composer_dir)?;

        let suffix = self.get_suffix();

        // Sort packages by dependency weight for reproducible output
        let sorted_packages = sort_packages_by_dependency(packages);

        // Collect exclude-from-classmap patterns from all packages
        let exclude_patterns = self.collect_exclude_patterns(&sorted_packages, root_autoload);

        // Collect autoload data from all packages
        // Use BTreeMap for sorted output
        let mut psr4: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut psr0: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut classmap: BTreeMap<String, String> = BTreeMap::new();
        // Files are stored as (identifier, path) pairs - order matters!
        let mut files: Vec<(String, String)> = Vec::new();

        // Process package autoloads in sorted order (dependencies first)
        // Skip metapackages as they have no files to autoload
        for pkg in &sorted_packages {
            if pkg.is_metapackage() {
                continue;
            }
            self.process_autoload(&pkg.autoload, &pkg.install_path, &pkg.name, &mut psr4, &mut psr0, &mut classmap, &mut files, &exclude_patterns)?;
        }

        // Process root autoload last (root overrides)
        if let Some(autoload) = root_autoload {
            self.process_autoload(autoload, "", "__root__", &mut psr4, &mut psr0, &mut classmap, &mut files, &exclude_patterns)?;
        }

        // Generate authoritative classmap if optimizing
        if self.config.optimize || self.config.authoritative {
            self.generate_optimized_classmap(&psr4, &psr0, &mut classmap, &exclude_patterns)?;
        }

        // Add Composer\InstalledVersions to classmap
        classmap.insert(
            "Composer\\InstalledVersions".to_string(),
            "$vendorDir . '/composer/InstalledVersions.php'".to_string(),
        );

        // Generate files
        self.generate_autoload_php(&composer_dir, &suffix)?;
        self.generate_autoload_real(&composer_dir, &suffix, !files.is_empty())?;
        self.generate_autoload_static(&composer_dir, &suffix, &psr4, &psr0, &classmap, &files)?;
        self.generate_autoload_psr4(&composer_dir, &psr4)?;
        self.generate_autoload_namespaces(&composer_dir, &psr0)?;
        self.generate_autoload_classmap(&composer_dir, &classmap)?;
        if !files.is_empty() {
            self.generate_autoload_files(&composer_dir, &files)?;
        }
        self.generate_platform_check(&composer_dir)?;
        self.generate_class_loader(&composer_dir)?;
        self.generate_installed_versions(&composer_dir)?;
        self.generate_installed_php(&composer_dir, &sorted_packages, root_package)?;

        Ok(())
    }

    /// Process a package's autoload configuration
    fn process_autoload(
        &self,
        autoload: &Autoload,
        install_path: &str,
        package_name: &str,
        psr4: &mut BTreeMap<String, Vec<String>>,
        psr0: &mut BTreeMap<String, Vec<String>>,
        classmap: &mut BTreeMap<String, String>,
        files: &mut Vec<(String, String)>,
        exclude_patterns: &[Regex],
    ) -> Result<()> {
        let is_root = install_path.is_empty();

        // PSR-4
        for (namespace, paths) in &autoload.psr4 {
            // Normalize namespace - strip leading backslash
            let ns = namespace.trim_start_matches('\\').to_string();
            let entry = psr4.entry(ns).or_default();
            for path in paths.as_vec() {
                let full_path = self.get_path_code(install_path, &path, is_root);
                entry.push(full_path);
            }
        }

        // PSR-0
        for (namespace, paths) in &autoload.psr0 {
            let ns = namespace.trim_start_matches('\\').to_string();
            let entry = psr0.entry(ns).or_default();
            for path in paths.as_vec() {
                let full_path = self.get_path_code(install_path, &path, is_root);
                entry.push(full_path);
            }
        }

        // Classmap
        for path in &autoload.classmap {
            let full_path = if is_root {
                self.config.base_dir.join(path)
            } else {
                self.config.vendor_dir.join(install_path).join(path)
            };
            let classes = self.classmap_generator.generate_with_excludes(&full_path, exclude_patterns)?;
            for (class_name, file_path) in classes {
                let path_code = self.path_to_code(&file_path);
                classmap.insert(class_name, path_code);
            }
        }

        // Files - compute identifier as md5(package_name:path)
        for path in &autoload.files {
            let file_identifier = Self::compute_file_identifier(package_name, path);
            let full_path = self.get_path_code(install_path, path, is_root);
            files.push((file_identifier, full_path));
        }

        Ok(())
    }

    /// Convert a path to PHP code reference ($vendorDir or $baseDir)
    /// This format is used for autoload_psr4.php, autoload_namespaces.php, etc.
    fn get_path_code(&self, install_path: &str, path: &str, is_root: bool) -> String {
        let path = path.trim_end_matches('/');
        if is_root {
            if path.is_empty() || path == "." {
                "$baseDir . '/'".to_string()
            } else {
                format!("$baseDir . '/{}'", path)
            }
        } else {
            let full_path = if path.is_empty() {
                install_path.to_string()
            } else {
                format!("{}/{}", install_path, path)
            };
            format!("$vendorDir . '/{}'", full_path)
        }
    }

    /// Convert an absolute PathBuf to PHP code reference
    fn path_to_code(&self, path: &PathBuf) -> String {
        let path_str = path.to_string_lossy();

        // Check if path is under vendor dir
        let vendor_path = self.config.vendor_dir.canonicalize().unwrap_or_else(|_| self.config.vendor_dir.clone());
        let base_path = self.config.base_dir.canonicalize().unwrap_or_else(|_| self.config.base_dir.clone());

        if let Ok(canonical) = path.canonicalize() {
            if let Ok(rel) = canonical.strip_prefix(&vendor_path) {
                return format!("$vendorDir . '/{}'", rel.display());
            }
            if let Ok(rel) = canonical.strip_prefix(&base_path) {
                return format!("$baseDir . '/{}'", rel.display());
            }
        }

        // Fallback - try without canonicalize
        if let Ok(rel) = path.strip_prefix(&self.config.vendor_dir) {
            return format!("$vendorDir . '/{}'", rel.display());
        }
        if let Ok(rel) = path.strip_prefix(&self.config.base_dir) {
            return format!("$baseDir . '/{}'", rel.display());
        }

        // Last resort - use $baseDir with the path
        format!("$baseDir . '/{}'", path_str)
    }

    /// Generate optimized classmap from PSR-4/PSR-0 directories
    fn generate_optimized_classmap(
        &self,
        psr4: &BTreeMap<String, Vec<String>>,
        psr0: &BTreeMap<String, Vec<String>>,
        classmap: &mut BTreeMap<String, String>,
        exclude_patterns: &[Regex],
    ) -> Result<()> {
        // Scan PSR-4 directories
        for paths in psr4.values() {
            for path_code in paths {
                // Extract actual path from code like "$vendorDir . '/symfony/console'"
                if let Some(path) = self.extract_path_from_code(path_code) {
                    let classes = self.classmap_generator.generate_with_excludes(Path::new(&path), exclude_patterns)?;
                    for (class_name, file_path) in classes {
                        let code = self.path_to_code(&file_path);
                        classmap.insert(class_name, code);
                    }
                }
            }
        }

        // Scan PSR-0 directories
        for paths in psr0.values() {
            for path_code in paths {
                if let Some(path) = self.extract_path_from_code(path_code) {
                    let classes = self.classmap_generator.generate_with_excludes(Path::new(&path), exclude_patterns)?;
                    for (class_name, file_path) in classes {
                        let code = self.path_to_code(&file_path);
                        classmap.insert(class_name, code);
                    }
                }
            }
        }

        Ok(())
    }

    /// Extract actual filesystem path from PHP code like "$vendorDir . '/path'"
    fn extract_path_from_code(&self, code: &str) -> Option<String> {
        if code.starts_with("$vendorDir") {
            // Extract path after "$vendorDir . '"
            let parts: Vec<&str> = code.splitn(2, "'").collect();
            if parts.len() >= 2 {
                let rel_path = parts[1].trim_end_matches('\'');
                return Some(self.config.vendor_dir.join(rel_path).to_string_lossy().to_string());
            }
        } else if code.starts_with("$baseDir") {
            let parts: Vec<&str> = code.splitn(2, "'").collect();
            if parts.len() >= 2 {
                let rel_path = parts[1].trim_end_matches('\'');
                return Some(self.config.base_dir.join(rel_path).to_string_lossy().to_string());
            }
        }
        None
    }

    /// Generate vendor/autoload.php
    fn generate_autoload_php(&self, _composer_dir: &Path, suffix: &str) -> Result<()> {
        let content = format!(r#"<?php

// autoload.php @generated by Composer

if (PHP_VERSION_ID < 50600) {{
    if (!headers_sent()) {{
        header('HTTP/1.1 500 Internal Server Error');
    }}
    $err = 'Composer 2.3.0 dropped support for autoloading on PHP <5.6 and you are running '.PHP_VERSION.', please upgrade PHP or use Composer 2.2 LTS via "composer self-update --2.2". Aborting.'.PHP_EOL;
    if (!ini_get('display_errors')) {{
        if (PHP_SAPI === 'cli' || PHP_SAPI === 'phpdbg') {{
            fwrite(STDERR, $err);
        }} elseif (!headers_sent()) {{
            echo $err;
        }}
    }}
    throw new RuntimeException($err);
}}

require_once __DIR__ . '/composer/autoload_real.php';

return ComposerAutoloaderInit{suffix}::getLoader();
"#);

        let autoload_path = self.config.vendor_dir.join("autoload.php");
        std::fs::write(autoload_path, content)?;
        Ok(())
    }

    /// Generate vendor/composer/autoload_real.php
    fn generate_autoload_real(&self, composer_dir: &Path, suffix: &str, has_files: bool) -> Result<()> {
        let apcu_prefix = if self.config.apcu {
            format!("        $loader->setApcuPrefix('ComposerAutoloader{}');\n", suffix)
        } else {
            String::new()
        };

        let authoritative = if self.config.authoritative {
            "        $loader->setClassMapAuthoritative(true);\n".to_string()
        } else {
            String::new()
        };

        let files_loader = if has_files {
            format!(r#"
        $filesToLoad = \Composer\Autoload\ComposerStaticInit{suffix}::$files;
        $requireFile = \Closure::bind(static function ($fileIdentifier, $file) {{
            if (empty($GLOBALS['__composer_autoload_files'][$fileIdentifier])) {{
                $GLOBALS['__composer_autoload_files'][$fileIdentifier] = true;

                require $file;
            }}
        }}, null, null);
        foreach ($filesToLoad as $fileIdentifier => $file) {{
            $requireFile($fileIdentifier, $file);
        }}
"#)
        } else {
            String::new()
        };

        let content = format!(r#"<?php

// autoload_real.php @generated by Composer

class ComposerAutoloaderInit{suffix}
{{
    private static $loader;

    public static function loadClassLoader($class)
    {{
        if ('Composer\Autoload\ClassLoader' === $class) {{
            require __DIR__ . '/ClassLoader.php';
        }}
    }}

    /**
     * @return \Composer\Autoload\ClassLoader
     */
    public static function getLoader()
    {{
        if (null !== self::$loader) {{
            return self::$loader;
        }}

        require __DIR__ . '/platform_check.php';

        spl_autoload_register(array('ComposerAutoloaderInit{suffix}', 'loadClassLoader'), true, true);
        self::$loader = $loader = new \Composer\Autoload\ClassLoader(\dirname(__DIR__));
        spl_autoload_unregister(array('ComposerAutoloaderInit{suffix}', 'loadClassLoader'));

        require __DIR__ . '/autoload_static.php';
        call_user_func(\Composer\Autoload\ComposerStaticInit{suffix}::getInitializer($loader));

        $loader->register(true);
{apcu_prefix}{authoritative}{files_loader}
        return $loader;
    }}
}}
"#);

        std::fs::write(composer_dir.join("autoload_real.php"), content)?;
        Ok(())
    }

    /// Convert $vendorDir/$baseDir paths to __DIR__ format for static file
    fn to_static_path(path: &str) -> String {
        if path.starts_with("$vendorDir") {
            // $vendorDir . '/x' => __DIR__ . '/..' . '/x'
            path.replace("$vendorDir", "__DIR__ . '/..'")
        } else if path.starts_with("$baseDir") {
            // $baseDir . '/x' => __DIR__ . '/../..' . '/x'
            path.replace("$baseDir", "__DIR__ . '/../..'")
        } else {
            path.to_string()
        }
    }

    /// Generate vendor/composer/autoload_static.php
    fn generate_autoload_static(
        &self,
        composer_dir: &Path,
        suffix: &str,
        psr4: &BTreeMap<String, Vec<String>>,
        psr0: &BTreeMap<String, Vec<String>>,
        classmap: &BTreeMap<String, String>,
        files: &[(String, String)],
    ) -> Result<()> {
        let mut content = format!(r#"<?php

// autoload_static.php @generated by Composer

namespace Composer\Autoload;

class ComposerStaticInit{suffix}
{{
"#);

        // Generate files array if present
        if !files.is_empty() {
            content.push_str("    public static $files = array (\n");
            for (identifier, path) in files {
                content.push_str(&format!("        '{}' => {},\n", identifier, Self::to_static_path(path)));
            }
            content.push_str("    );\n\n");
        }

        // Generate PSR-4 prefix lengths grouped by first character
        // Sorted in descending order by namespace (krsort equivalent)
        let mut psr4_vec: Vec<_> = psr4.iter().collect();
        psr4_vec.sort_by(|a, b| b.0.cmp(a.0)); // Reverse sort

        if !psr4.is_empty() {
            // Group by first character
            let mut by_first_char: BTreeMap<char, Vec<(&String, usize)>> = BTreeMap::new();
            for (namespace, _) in &psr4_vec {
                let first_char = namespace.chars().next().unwrap_or('_');
                by_first_char.entry(first_char)
                    .or_default()
                    .push((namespace, namespace.len()));
            }

            content.push_str("    public static $prefixLengthsPsr4 = array (\n");
            // Sort by first char descending
            let mut char_entries: Vec<_> = by_first_char.iter().collect();
            char_entries.sort_by(|a, b| b.0.cmp(a.0));

            for (first_char, namespaces) in char_entries {
                content.push_str(&format!("        '{}' =>\n        array (\n", first_char));
                for (ns, len) in namespaces {
                    let ns_escaped = ns.replace('\\', "\\\\");
                    content.push_str(&format!("            '{}' => {},\n", ns_escaped, len));
                }
                content.push_str("        ),\n");
            }
            content.push_str("    );\n\n");

            // Generate PSR-4 prefix directories
            content.push_str("    public static $prefixDirsPsr4 = array (\n");
            for (namespace, paths) in &psr4_vec {
                let ns_escaped = namespace.replace('\\', "\\\\");
                content.push_str(&format!("        '{}' =>\n        array (\n", ns_escaped));
                for (i, path) in paths.iter().enumerate() {
                    content.push_str(&format!("            {} => {},\n", i, Self::to_static_path(path)));
                }
                content.push_str("        ),\n");
            }
            content.push_str("    );\n\n");
        }

        // Generate PSR-0 prefixes if present
        if !psr0.is_empty() {
            let mut psr0_vec: Vec<_> = psr0.iter().collect();
            psr0_vec.sort_by(|a, b| b.0.cmp(a.0));

            // Group by first character
            let mut by_first_char: BTreeMap<char, Vec<(&String, &Vec<String>)>> = BTreeMap::new();
            for (namespace, paths) in &psr0_vec {
                let first_char = namespace.chars().next().unwrap_or('_');
                by_first_char.entry(first_char)
                    .or_default()
                    .push((namespace, paths));
            }

            content.push_str("    public static $prefixesPsr0 = array (\n");
            let mut char_entries: Vec<_> = by_first_char.iter().collect();
            char_entries.sort_by(|a, b| b.0.cmp(a.0));

            for (first_char, namespaces) in char_entries {
                content.push_str(&format!("        '{}' =>\n        array (\n", first_char));
                for (ns, paths) in namespaces {
                    let ns_escaped = ns.replace('\\', "\\\\");
                    content.push_str(&format!("            '{}' =>\n            array (\n", ns_escaped));
                    for (i, path) in paths.iter().enumerate() {
                        content.push_str(&format!("                {} => {},\n", i, Self::to_static_path(path)));
                    }
                    content.push_str("            ),\n");
                }
                content.push_str("        ),\n");
            }
            content.push_str("    );\n\n");
        }

        // Generate classmap
        content.push_str("    public static $classMap = array (\n");
        for (class, path) in classmap {
            let class_escaped = class.replace('\\', "\\\\");
            content.push_str(&format!("        '{}' => {},\n", class_escaped, Self::to_static_path(path)));
        }
        content.push_str("    );\n\n");

        // Generate initializer
        let mut initializer_content = String::new();
        if !psr4.is_empty() {
            initializer_content.push_str(&format!(
                "            $loader->prefixLengthsPsr4 = ComposerStaticInit{}::$prefixLengthsPsr4;\n",
                suffix
            ));
            initializer_content.push_str(&format!(
                "            $loader->prefixDirsPsr4 = ComposerStaticInit{}::$prefixDirsPsr4;\n",
                suffix
            ));
        }
        if !psr0.is_empty() {
            initializer_content.push_str(&format!(
                "            $loader->prefixesPsr0 = ComposerStaticInit{}::$prefixesPsr0;\n",
                suffix
            ));
        }
        initializer_content.push_str(&format!(
            "            $loader->classMap = ComposerStaticInit{}::$classMap;\n",
            suffix
        ));

        content.push_str(&format!(r#"    public static function getInitializer(ClassLoader $loader)
    {{
        return \Closure::bind(function () use ($loader) {{
{}
        }}, null, ClassLoader::class);
    }}
}}
"#, initializer_content));

        std::fs::write(composer_dir.join("autoload_static.php"), content)?;
        Ok(())
    }

    /// Generate vendor/composer/autoload_psr4.php
    fn generate_autoload_psr4(&self, composer_dir: &Path, psr4: &BTreeMap<String, Vec<String>>) -> Result<()> {
        // Sort in descending order like Composer does (krsort)
        let mut psr4_vec: Vec<_> = psr4.iter().collect();
        psr4_vec.sort_by(|a, b| b.0.cmp(a.0));

        let mut entries = Vec::new();
        for (namespace, paths) in psr4_vec {
            let ns_escaped = namespace.replace('\\', "\\\\");
            let paths_str: Vec<String> = paths.iter()
                .map(|p| p.clone())
                .collect();

            entries.push(format!(
                "    '{}' => array({})",
                ns_escaped,
                paths_str.join(", ")
            ));
        }

        let content = format!(r#"<?php

// autoload_psr4.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
{},
);
"#, entries.join(",\n"));

        std::fs::write(composer_dir.join("autoload_psr4.php"), content)?;
        Ok(())
    }

    /// Generate vendor/composer/autoload_namespaces.php (PSR-0)
    fn generate_autoload_namespaces(&self, composer_dir: &Path, psr0: &BTreeMap<String, Vec<String>>) -> Result<()> {
        let mut psr0_vec: Vec<_> = psr0.iter().collect();
        psr0_vec.sort_by(|a, b| b.0.cmp(a.0));

        let mut entries = Vec::new();
        for (namespace, paths) in psr0_vec {
            let ns_escaped = namespace.replace('\\', "\\\\");
            let paths_str: Vec<String> = paths.iter()
                .map(|p| p.clone())
                .collect();

            entries.push(format!(
                "    '{}' => array({})",
                ns_escaped,
                paths_str.join(", ")
            ));
        }

        let entries_str = if entries.is_empty() {
            String::new()
        } else {
            format!("{},\n", entries.join(",\n"))
        };

        let content = format!(r#"<?php

// autoload_namespaces.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
{});
"#, entries_str);

        std::fs::write(composer_dir.join("autoload_namespaces.php"), content)?;
        Ok(())
    }

    /// Generate vendor/composer/autoload_classmap.php
    fn generate_autoload_classmap(&self, composer_dir: &Path, classmap: &BTreeMap<String, String>) -> Result<()> {
        let entries: Vec<String> = classmap.iter().map(|(class, path)| {
            format!("    '{}' => {}", class.replace('\\', "\\\\"), path)
        }).collect();

        let entries_str = if entries.is_empty() {
            String::new()
        } else {
            format!("{},\n", entries.join(",\n"))
        };

        let content = format!(r#"<?php

// autoload_classmap.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
{});
"#, entries_str);

        std::fs::write(composer_dir.join("autoload_classmap.php"), content)?;
        Ok(())
    }

    /// Generate vendor/composer/autoload_files.php
    fn generate_autoload_files(&self, composer_dir: &Path, files: &[(String, String)]) -> Result<()> {
        let entries: Vec<String> = files.iter()
            .map(|(identifier, path)| format!("    '{}' => {}", identifier, path))
            .collect();

        let entries_str = if entries.is_empty() {
            String::new()
        } else {
            format!("{},\n", entries.join(",\n"))
        };

        let content = format!(r#"<?php

// autoload_files.php @generated by Composer

$vendorDir = dirname(__DIR__);
$baseDir = dirname($vendorDir);

return array(
{});
"#, entries_str);

        std::fs::write(composer_dir.join("autoload_files.php"), content)?;
        Ok(())
    }

    /// Generate vendor/composer/platform_check.php
    fn generate_platform_check(&self, composer_dir: &Path) -> Result<()> {
        // Generate a minimal platform check file
        // In a full implementation, this would check PHP version and required extensions
        let content = r#"<?php

// platform_check.php @generated by Composer

$issues = array();

if (!(PHP_VERSION_ID >= 80100)) {
    $issues[] = 'Your Composer dependencies require a PHP version ">= 8.1.0". You are running ' . PHP_VERSION . '.';
}

if ($issues) {
    if (!headers_sent()) {
        header('HTTP/1.1 500 Internal Server Error');
    }
    if (!ini_get('display_errors')) {
        if (PHP_SAPI === 'cli' || PHP_SAPI === 'phpdbg') {
            fwrite(STDERR, 'Composer detected issues in your platform:' . PHP_EOL.PHP_EOL . implode(PHP_EOL, $issues) . PHP_EOL.PHP_EOL);
        } elseif (!headers_sent()) {
            echo 'Composer detected issues in your platform:' . PHP_EOL.PHP_EOL . str_replace('You are running '.PHP_VERSION.'.', '', implode(PHP_EOL, $issues)) . PHP_EOL.PHP_EOL;
        }
    }
    throw new \RuntimeException(
        'Composer detected issues in your platform: ' . implode(' ', $issues)
    );
}
"#;

        std::fs::write(composer_dir.join("platform_check.php"), content)?;
        Ok(())
    }

    /// Generate vendor/composer/InstalledVersions.php
    fn generate_installed_versions(&self, composer_dir: &Path) -> Result<()> {
        // Copy the InstalledVersions.php template
        let content = include_str!("InstalledVersions.php.template");
        std::fs::write(composer_dir.join("InstalledVersions.php"), content)?;
        Ok(())
    }

    /// Generate vendor/composer/ClassLoader.php
    fn generate_class_loader(&self, composer_dir: &Path) -> Result<()> {
        // This is the standard Composer ClassLoader - a simplified version
        let content = include_str!("ClassLoader.php.template");
        std::fs::write(composer_dir.join("ClassLoader.php"), content)?;
        Ok(())
    }

    /// Compute MD5 hash for file identifier (package_name:path)
    /// This matches Composer's behavior
    fn compute_file_identifier(package_name: &str, path: &str) -> String {
        let mut hasher = Md5::new();
        hasher.update(format!("{}:{}", package_name, path).as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Generate vendor/composer/installed.php
    fn generate_installed_php(&self, composer_dir: &Path, packages: &[PackageAutoload], root_package: Option<&RootPackageInfo>) -> Result<()> {
        let mut versions: BTreeMap<String, PackageVersionEntry> = BTreeMap::new();

        // Add all installed packages to versions
        for pkg in packages {
            // Metapackages have no install path (they have no files)
            let install_path = if pkg.is_metapackage() {
                None
            } else {
                Some(format!("__DIR__ . '/../{}'", pkg.install_path))
            };

            let entry = PackageVersionEntry {
                pretty_version: pkg.pretty_version.clone(),
                version: pkg.version.clone(),
                reference: pkg.reference.clone(),
                package_type: Some(pkg.package_type.clone()),
                install_path,
                aliases: pkg.aliases.clone(),
                dev_requirement: pkg.dev_requirement,
                replaced: Vec::new(),
                provided: Vec::new(),
            };
            versions.insert(pkg.name.clone(), entry);
        }

        // Process replaced and provided packages
        for pkg in packages {
            let is_dev = pkg.dev_requirement;

            // Handle replaced packages
            for (replaced_name, version_constraint) in &pkg.replaces {
                // Skip platform packages
                if Self::is_platform_package(replaced_name) {
                    continue;
                }

                let replaced_version = if version_constraint == "self.version" {
                    pkg.pretty_version.clone().unwrap_or_default()
                } else {
                    version_constraint.clone()
                };

                if let Some(entry) = versions.get_mut(replaced_name) {
                    // Package exists, add to its replaced list
                    if !entry.replaced.contains(&replaced_version) {
                        entry.replaced.push(replaced_version);
                    }
                    // Only mark as non-dev if this package is non-dev
                    if !is_dev {
                        entry.dev_requirement = false;
                    }
                } else {
                    // Virtual package - create entry with just replaced info
                    versions.insert(replaced_name.clone(), PackageVersionEntry {
                        pretty_version: None,
                        version: None,
                        reference: None,
                        package_type: None,
                        install_path: None,
                        aliases: Vec::new(),
                        dev_requirement: is_dev,
                        replaced: vec![replaced_version],
                        provided: Vec::new(),
                    });
                }
            }

            // Handle provided packages
            for (provided_name, version_constraint) in &pkg.provides {
                // Skip platform packages
                if Self::is_platform_package(provided_name) {
                    continue;
                }

                let provided_version = if version_constraint == "self.version" {
                    pkg.pretty_version.clone().unwrap_or_default()
                } else {
                    version_constraint.clone()
                };

                if let Some(entry) = versions.get_mut(provided_name) {
                    if !entry.provided.contains(&provided_version) {
                        entry.provided.push(provided_version);
                    }
                    if !is_dev {
                        entry.dev_requirement = false;
                    }
                } else {
                    versions.insert(provided_name.clone(), PackageVersionEntry {
                        pretty_version: None,
                        version: None,
                        reference: None,
                        package_type: None,
                        install_path: None,
                        aliases: Vec::new(),
                        dev_requirement: is_dev,
                        replaced: Vec::new(),
                        provided: vec![provided_version],
                    });
                }
            }
        }

        // Sort replaced/provided arrays
        for entry in versions.values_mut() {
            entry.replaced.sort();
            entry.provided.sort();
            entry.aliases.sort();
        }

        // Build root package entry
        let (root_name, root_pretty_version, root_version, root_reference, root_type, root_aliases, root_dev) =
            if let Some(root) = root_package {
                (
                    root.name.clone(),
                    root.pretty_version.clone(),
                    root.version.clone(),
                    root.reference.clone(),
                    root.package_type.clone(),
                    root.aliases.clone(),
                    root.dev_mode,
                )
            } else {
                (
                    "__root__".to_string(),
                    "dev-main".to_string(),
                    "dev-main".to_string(),
                    None,
                    "library".to_string(),
                    Vec::new(),
                    true,
                )
            };

        // Also add root package to versions (Composer does this)
        versions.insert(root_name.clone(), PackageVersionEntry {
            pretty_version: Some(root_pretty_version.clone()),
            version: Some(root_version.clone()),
            reference: root_reference.clone(),
            package_type: Some(root_type.clone()),
            install_path: Some("__DIR__ . '/../../'".to_string()),
            aliases: root_aliases.clone(),
            dev_requirement: false,
            replaced: Vec::new(),
            provided: Vec::new(),
        });

        // Generate the PHP code
        let mut content = String::from("<?php return array(\n");

        // Root section
        content.push_str("    'root' => array(\n");
        content.push_str(&format!("        'name' => {},\n", Self::php_string(&root_name)));
        content.push_str(&format!("        'pretty_version' => {},\n", Self::php_string(&root_pretty_version)));
        content.push_str(&format!("        'version' => {},\n", Self::php_string(&root_version)));
        content.push_str(&format!("        'reference' => {},\n", Self::php_value_or_null(&root_reference)));
        content.push_str(&format!("        'type' => {},\n", Self::php_string(&root_type)));
        content.push_str("        'install_path' => __DIR__ . '/../../',\n");
        content.push_str(&format!("        'aliases' => {},\n", Self::php_string_array(&root_aliases)));
        content.push_str(&format!("        'dev' => {},\n", if root_dev { "true" } else { "false" }));
        content.push_str("    ),\n");

        // Versions section
        content.push_str("    'versions' => array(\n");
        for (name, entry) in &versions {
            content.push_str(&format!("        {} => array(\n", Self::php_string(name)));

            if let Some(ref pv) = entry.pretty_version {
                content.push_str(&format!("            'pretty_version' => {},\n", Self::php_string(pv)));
            }
            if let Some(ref v) = entry.version {
                content.push_str(&format!("            'version' => {},\n", Self::php_string(v)));
            }
            if entry.pretty_version.is_some() || entry.version.is_some() {
                content.push_str(&format!("            'reference' => {},\n", Self::php_value_or_null(&entry.reference)));
            }
            if let Some(ref t) = entry.package_type {
                content.push_str(&format!("            'type' => {},\n", Self::php_string(t)));
            }
            if let Some(ref ip) = entry.install_path {
                content.push_str(&format!("            'install_path' => {},\n", ip));
            }
            if !entry.aliases.is_empty() || entry.pretty_version.is_some() {
                content.push_str(&format!("            'aliases' => {},\n", Self::php_string_array(&entry.aliases)));
            }
            content.push_str(&format!("            'dev_requirement' => {},\n", if entry.dev_requirement { "true" } else { "false" }));
            if !entry.replaced.is_empty() {
                content.push_str(&format!("            'replaced' => {},\n", Self::php_string_array(&entry.replaced)));
            }
            if !entry.provided.is_empty() {
                content.push_str(&format!("            'provided' => {},\n", Self::php_string_array(&entry.provided)));
            }
            content.push_str("        ),\n");
        }
        content.push_str("    ),\n");
        content.push_str(");\n");

        std::fs::write(composer_dir.join("installed.php"), content)?;
        Ok(())
    }

    /// Check if a package name is a platform package (php, ext-*, lib-*)
    fn is_platform_package(name: &str) -> bool {
        name == "php" || name == "php-64bit" || name == "hhvm" ||
        name.starts_with("ext-") || name.starts_with("lib-") ||
        name.starts_with("composer-")
    }

    /// Convert a string to PHP string literal
    fn php_string(s: &str) -> String {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }

    /// Convert an Option<String> to PHP value or null
    fn php_value_or_null(opt: &Option<String>) -> String {
        match opt {
            Some(s) => Self::php_string(s),
            None => "NULL".to_string(),
        }
    }

    /// Convert a Vec<String> to PHP array
    fn php_string_array(arr: &[String]) -> String {
        if arr.is_empty() {
            "array()".to_string()
        } else {
            let items: Vec<String> = arr.iter().map(|s| Self::php_string(s)).collect();
            format!("array({})", items.join(", "))
        }
    }
}

/// Internal structure for building installed.php version entries
#[derive(Debug, Clone)]
struct PackageVersionEntry {
    pretty_version: Option<String>,
    version: Option<String>,
    reference: Option<String>,
    package_type: Option<String>,
    install_path: Option<String>,
    aliases: Vec<String>,
    dev_requirement: bool,
    replaced: Vec<String>,
    provided: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_autoload_config_default() {
        let config = AutoloadConfig::default();
        assert_eq!(config.vendor_dir, PathBuf::from("vendor"));
        assert!(!config.optimize);
        assert!(!config.apcu);
    }

    #[test]
    fn test_generate_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = AutoloadConfig {
            vendor_dir: temp_dir.path().join("vendor"),
            ..Default::default()
        };

        let generator = AutoloadGenerator::new(config);
        let result = generator.generate(&[], None, None);

        assert!(result.is_ok());
        assert!(temp_dir.path().join("vendor/autoload.php").exists());
        assert!(temp_dir.path().join("vendor/composer/autoload_real.php").exists());
    }

    #[test]
    fn test_generate_installed_php_with_packages() {
        let temp_dir = TempDir::new().unwrap();
        let config = AutoloadConfig {
            vendor_dir: temp_dir.path().join("vendor"),
            base_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let packages = vec![
            PackageAutoload {
                name: "vendor/package1".to_string(),
                install_path: "vendor/package1".to_string(),
                pretty_version: Some("1.0.0".to_string()),
                version: Some("1.0.0.0".to_string()),
                reference: Some("abc123".to_string()),
                package_type: "library".to_string(),
                dev_requirement: false,
                replaces: IndexMap::new(),
                provides: IndexMap::new(),
                ..Default::default()
            },
            PackageAutoload {
                name: "vendor/package2".to_string(),
                install_path: "vendor/package2".to_string(),
                pretty_version: Some("2.0.0".to_string()),
                version: Some("2.0.0.0".to_string()),
                reference: Some("def456".to_string()),
                package_type: "library".to_string(),
                dev_requirement: true,
                replaces: IndexMap::new(),
                provides: IndexMap::new(),
                ..Default::default()
            },
        ];

        let root = RootPackageInfo {
            name: "my/project".to_string(),
            pretty_version: "dev-main".to_string(),
            version: "dev-main".to_string(),
            reference: None,
            package_type: "project".to_string(),
            aliases: Vec::new(),
            dev_mode: true,
        };

        let generator = AutoloadGenerator::new(config);
        let result = generator.generate(&packages, None, Some(&root));

        assert!(result.is_ok());

        // Check installed.php was created
        let installed_path = temp_dir.path().join("vendor/composer/installed.php");
        assert!(installed_path.exists());

        // Read and verify content
        let content = std::fs::read_to_string(&installed_path).unwrap();
        assert!(content.contains("'my/project'"));
        assert!(content.contains("'vendor/package1'"));
        assert!(content.contains("'vendor/package2'"));
        assert!(content.contains("'1.0.0'"));
        assert!(content.contains("'abc123'"));
        assert!(content.contains("'dev_requirement' => false"));
        assert!(content.contains("'dev_requirement' => true"));
    }

    #[test]
    fn test_generate_installed_php_with_provides_and_replaces() {
        let temp_dir = TempDir::new().unwrap();
        let config = AutoloadConfig {
            vendor_dir: temp_dir.path().join("vendor"),
            base_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let mut replaces = IndexMap::new();
        replaces.insert("old/package".to_string(), "1.0.0".to_string());

        let mut provides = IndexMap::new();
        provides.insert("psr/log-implementation".to_string(), "1.0.0".to_string());

        let packages = vec![
            PackageAutoload {
                name: "monolog/monolog".to_string(),
                install_path: "monolog/monolog".to_string(),
                pretty_version: Some("2.0.0".to_string()),
                version: Some("2.0.0.0".to_string()),
                reference: None,
                package_type: "library".to_string(),
                dev_requirement: false,
                replaces,
                provides,
                ..Default::default()
            },
        ];

        let generator = AutoloadGenerator::new(config);
        let result = generator.generate(&packages, None, None);

        assert!(result.is_ok());

        let installed_path = temp_dir.path().join("vendor/composer/installed.php");
        let content = std::fs::read_to_string(&installed_path).unwrap();

        // Check that provides and replaces entries are present
        assert!(content.contains("'psr/log-implementation'"));
        assert!(content.contains("'provided'"));
        assert!(content.contains("'old/package'"));
        assert!(content.contains("'replaced'"));
    }
}
