//! Composer bin plugin - isolate bin dependencies.
//!
//! This is a native Rust port of bamarni/composer-bin-plugin.
//! When forward-command is enabled, install/update commands are
//! automatically forwarded to all bin namespaces in vendor-bin/.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use crate::composer::Composer;
use crate::event::{ComposerEvent, EventListener, EventType, PostAutoloadDumpEvent};
use crate::json::ComposerJson;
use crate::package::Package;
use crate::Result;

/// The package name that triggers this plugin.
pub const PACKAGE_NAME: &str = "bamarni/composer-bin-plugin";

/// Configuration for the bin plugin from composer.json extra.bamarni-bin
#[derive(Debug, Clone)]
pub struct BinConfig {
    /// Whether to create bin links in the main vendor/bin directory
    pub bin_links: bool,
    /// Target directory for bin namespaces (default: vendor-bin)
    pub target_directory: String,
    /// Whether to forward install/update commands to all namespaces
    pub forward_command: bool,
}

impl Default for BinConfig {
    fn default() -> Self {
        Self {
            bin_links: false,  // Default to false in 2.x behavior
            target_directory: "vendor-bin".to_string(),
            forward_command: false,
        }
    }
}

impl BinConfig {
    /// Parse config from composer.json extra field
    pub fn from_extra(extra: &serde_json::Value) -> Self {
        let bamarni_bin = extra.get("bamarni-bin");

        let mut config = Self::default();

        if let Some(obj) = bamarni_bin.and_then(|v| v.as_object()) {
            if let Some(bin_links) = obj.get("bin-links").and_then(|v| v.as_bool()) {
                config.bin_links = bin_links;
            }
            if let Some(target_dir) = obj.get("target-directory").and_then(|v| v.as_str()) {
                config.target_directory = target_dir.to_string();
            }
            if let Some(forward) = obj.get("forward-command").and_then(|v| v.as_bool()) {
                config.forward_command = forward;
            }
        }

        config
    }
}

/// Composer bin plugin - implements EventListener directly.
pub struct ComposerBinPlugin;

impl EventListener for ComposerBinPlugin {
    fn handle(&self, event: &dyn ComposerEvent, composer: &Composer) -> anyhow::Result<i32> {
        if event.event_type() != EventType::PostAutoloadDump {
            return Ok(0);
        }

        let Some(e) = event.as_any().downcast_ref::<PostAutoloadDumpEvent>() else {
            return Ok(0);
        };

        // Check if our package is installed
        let is_installed = e.packages.iter().any(|p| p.name == PACKAGE_NAME);
        if !is_installed {
            return Ok(0);
        }

        self.post_autoload_dump(
            &composer.vendor_dir(),
            &composer.working_dir,
            &composer.composer_json,
            &e.packages,
        )?;

        Ok(0)
    }

    fn priority(&self) -> i32 {
        -10
    }
}

impl ComposerBinPlugin {
    fn post_autoload_dump(
        &self,
        vendor_dir: &Path,
        project_dir: &Path,
        composer_json: &ComposerJson,
        _installed_packages: &[Arc<Package>],
    ) -> Result<()> {
        let config = BinConfig::from_extra(&composer_json.extra);

        // Only act if forward-command is enabled
        if !config.forward_command {
            return Ok(());
        }

        let vendor_bin_root = project_dir.join(&config.target_directory);

        if !vendor_bin_root.exists() {
            return Ok(());
        }

        // Find all namespace directories
        let namespaces: Vec<_> = std::fs::read_dir(&vendor_bin_root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        if namespaces.is_empty() {
            return Ok(());
        }

        // Get bin directory for bin-links
        let bin_dir = vendor_dir.join("bin");

        // Forward install command to all namespaces
        for entry in namespaces {
            let namespace_dir = entry.path();
            let namespace_name = entry.file_name().to_string_lossy().to_string();

            // Ensure composer.json exists
            let namespace_composer = namespace_dir.join("composer.json");
            if !namespace_composer.exists() {
                std::fs::write(&namespace_composer, "{}")?;
            }

            // Run phpx install in the namespace directory
            if let Ok(current_exe) = std::env::current_exe() {
                let status = Command::new(&current_exe)
                    .arg("install")
                    .arg("-d")
                    .arg(&namespace_dir)
                    .status();

                if let Err(e) = status {
                    eprintln!("Warning: Failed to run install in namespace {}: {}", namespace_name, e);
                }
            }

            // Create bin links if enabled
            if config.bin_links {
                create_bin_links(&namespace_dir, &bin_dir)?;
            }
        }

        Ok(())
    }
}

/// Create symlinks from namespace vendor/bin to main vendor/bin
fn create_bin_links(namespace_dir: &Path, main_bin_dir: &Path) -> Result<()> {
    let namespace_bin_dir = namespace_dir.join("vendor").join("bin");

    if !namespace_bin_dir.exists() {
        return Ok(());
    }

    // Ensure main bin dir exists
    std::fs::create_dir_all(main_bin_dir)?;

    // Create symlinks for each binary
    for entry in std::fs::read_dir(&namespace_bin_dir)? {
        let entry = entry?;
        let source = entry.path();
        let file_name = entry.file_name();
        let target = main_bin_dir.join(&file_name);

        // Skip if target already exists
        if target.exists() {
            continue;
        }

        // Create symlink
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&source, &target)?;
        }

        #[cfg(windows)]
        {
            // On Windows, copy instead of symlink for simplicity
            std::fs::copy(&source, &target)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bin_config_default() {
        let config = BinConfig::default();
        assert!(!config.bin_links);
        assert_eq!(config.target_directory, "vendor-bin");
        assert!(!config.forward_command);
    }

    #[test]
    fn test_bin_config_from_extra() {
        let extra = serde_json::json!({
            "bamarni-bin": {
                "bin-links": true,
                "target-directory": "tools",
                "forward-command": true
            }
        });

        let config = BinConfig::from_extra(&extra);
        assert!(config.bin_links);
        assert_eq!(config.target_directory, "tools");
        assert!(config.forward_command);
    }

    #[test]
    fn test_bin_config_partial_extra() {
        let extra = serde_json::json!({
            "bamarni-bin": {
                "forward-command": true
            }
        });

        let config = BinConfig::from_extra(&extra);
        assert!(!config.bin_links);  // default
        assert_eq!(config.target_directory, "vendor-bin");  // default
        assert!(config.forward_command);  // overridden
    }
}
