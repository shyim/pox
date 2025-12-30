use std::path::Path;
use std::fs;

use super::schema::ComposerJson;

/// Errors that can occur when loading composer.json
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("Failed to read file: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse JSON: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("Validation error: {0}")]
    Validation(String),
}

/// Load and parse a composer.json file
pub fn load_composer_json(path: &Path) -> Result<ComposerJson, LoadError> {
    let content = fs::read_to_string(path)?;
    parse_composer_json(&content)
}

/// Parse composer.json from a string
pub fn parse_composer_json(content: &str) -> Result<ComposerJson, LoadError> {
    let json: ComposerJson = serde_json::from_str(content)?;
    Ok(json)
}

/// Validate a composer.json structure
pub fn validate_composer_json(json: &ComposerJson) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    // Name validation
    if let Some(ref name) = json.name {
        if !is_valid_package_name(name) {
            errors.push(format!(
                "Invalid package name '{}'. Must be lowercase and match vendor/package format",
                name
            ));
        }
    }

    // Version validation (if specified)
    if let Some(ref version) = json.version {
        if version.is_empty() {
            errors.push("Version cannot be empty if specified".to_string());
        }
    }

    // Minimum stability validation
    let valid_stabilities = ["dev", "alpha", "beta", "rc", "stable"];
    if let Some(ref min_stability) = json.minimum_stability {
        if !valid_stabilities.contains(&min_stability.to_lowercase().as_str()) {
            errors.push(format!(
                "Invalid minimum-stability '{}'. Must be one of: {:?}",
                min_stability, valid_stabilities
            ));
        }
    }

    // Type validation
    let valid_types = ["library", "project", "metapackage", "composer-plugin"];
    if !valid_types.contains(&json.package_type.to_lowercase().as_str())
        && !json.package_type.starts_with("library")
    {
        // Allow custom types but warn (not an error)
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Check if a package name is valid
fn is_valid_package_name(name: &str) -> bool {
    // Must contain exactly one /
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() != 2 {
        return false;
    }

    let vendor = parts[0];
    let package = parts[1];

    // Both must be non-empty and lowercase
    if vendor.is_empty() || package.is_empty() {
        return false;
    }

    // Check for valid characters (alphanumeric, -, _, .)
    let is_valid_part = |s: &str| {
        s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
    };

    is_valid_part(vendor) && is_valid_part(package)
}

/// Write composer.json to a file
pub fn write_composer_json(path: &Path, json: &ComposerJson) -> Result<(), LoadError> {
    let content = serde_json::to_string_pretty(json)?;
    fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal() {
        let json = r#"{
            "name": "vendor/package",
            "require": {
                "php": ">=8.0"
            }
        }"#;

        let result = parse_composer_json(json).unwrap();
        assert_eq!(result.name, Some("vendor/package".to_string()));
        assert_eq!(result.require.get("php"), Some(&">=8.0".to_string()));
    }

    #[test]
    fn test_parse_full() {
        let json = r#"{
            "name": "vendor/package",
            "description": "A test package",
            "type": "library",
            "license": "MIT",
            "authors": [
                {
                    "name": "John Doe",
                    "email": "john@example.com"
                }
            ],
            "require": {
                "php": ">=8.0",
                "vendor/other": "^1.0"
            },
            "require-dev": {
                "phpunit/phpunit": "^10.0"
            },
            "autoload": {
                "psr-4": {
                    "Vendor\\Package\\": "src/"
                }
            }
        }"#;

        let result = parse_composer_json(json).unwrap();
        assert_eq!(result.name, Some("vendor/package".to_string()));
        assert_eq!(result.description, Some("A test package".to_string()));
        assert_eq!(result.package_type, "library");
        assert_eq!(result.authors.len(), 1);
        assert_eq!(result.require.len(), 2);
        assert_eq!(result.require_dev.len(), 1);
    }

    #[test]
    fn test_valid_package_name() {
        assert!(is_valid_package_name("vendor/package"));
        assert!(is_valid_package_name("my-vendor/my-package"));
        assert!(is_valid_package_name("vendor123/package456"));
        assert!(!is_valid_package_name("invalid"));
        assert!(!is_valid_package_name("Invalid/Package"));
        assert!(!is_valid_package_name("/package"));
        assert!(!is_valid_package_name("vendor/"));
    }

    #[test]
    fn test_validate_composer_json() {
        let mut json = ComposerJson::default();
        json.name = Some("vendor/package".to_string());
        json.minimum_stability = Some("stable".to_string());

        assert!(validate_composer_json(&json).is_ok());

        json.name = Some("InvalidName".to_string());
        assert!(validate_composer_json(&json).is_err());
    }

    #[test]
    fn test_branch_aliases() {
        let json = r#"{
            "name": "vendor/package",
            "extra": {
                "branch-alias": {
                    "dev-main": "1.0.x-dev",
                    "dev-2.x": "2.0.x-dev"
                }
            }
        }"#;

        let result = parse_composer_json(json).unwrap();
        let aliases = result.get_branch_aliases();

        // Should have parsed branch aliases
        assert!(!aliases.is_empty());
    }

    #[test]
    fn test_inline_alias() {
        // Test inline alias parsing from ComposerJson helper
        let result = ComposerJson::get_inline_alias("dev-main as 1.0.0");
        assert!(result.is_some());
        let (actual, alias) = result.unwrap();
        assert_eq!(actual, "dev-main");
        assert_eq!(alias, "1.0.0");

        // Test regular constraint (no alias)
        let result = ComposerJson::get_inline_alias("^1.0");
        assert!(result.is_none());
    }
}
